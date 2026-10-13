//! Background work: demo parsing and the sequential render queue. The engine
//! owns the [`Store`] (config, parse results and render jobs), scans demos from
//! disk, and loads saved analysis into memory on demand. Changes are reported
//! through a [`Notify`] sink so the Tauri layer can forward them as window events.

use crate::parser::DemoParser;
use crate::radar::{ensure_map_assets, MapAssets};
use crate::render::paths::{
    find_cs2_dir, find_steam_dir, replays_dir, resolve_tool_paths, PathOverrides, ToolPaths,
};
use crate::render::{
    doctor, render_highlights, run_setup, DoctorReport, RenderJobInput, RenderOptions, SetupTool,
};
use crate::replay::build_replay;
use crate::stats::build_parsed_demo;
use crate::stats::ParsedDemo;
use crate::store::{
    now, DemoMeta, DemoStatus, DemoSummary, JobOutput, JobStatus, RenderJob, Settings,
    SourceSnapshot, Store,
};
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

mod auto_attempts;
mod clip_media;
mod tool_access;
pub use clip_media::ClipMedia;

const PARSE_CONCURRENCY: usize = 3;

/// Events the UI cares about. Payloads are already JSON-serializable.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Event {
    DemoChanged {
        demo: DemoMeta,
    },
    AnalysisJobChanged {
        job: crate::scoring::queue::Job,
    },
    ScoringProgress {
        id: String,
        step: u8,
    },
    JobChanged {
        job: Box<RenderJob>,
    },
    SetupProgress {
        tool: SetupTool,
        progress: String,
    },
    SetupFinished {
        tool: SetupTool,
        ok: bool,
        installed: bool,
        cancelled: bool,
        error: Option<String>,
    },
}

pub trait Notify: Send + Sync + 'static {
    fn notify(&self, event: Event);
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    pub steam_dir: Option<PathBuf>,
    pub cs2_dir: Option<PathBuf>,
    pub replays_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupState {
    pub running: bool,
    pub stopping: bool,
    pub log: Vec<String>,
    pub progress: Option<String>,
    #[serde(skip)]
    cancel: Arc<AtomicBool>,
}

/// In-memory state of one demo: what the last scan saw plus the parse result.
#[derive(Clone)]
struct DemoEntry {
    meta: DemoMeta,
    parsed: Option<Arc<ParsedDemo>>,
    auto_complete: Option<bool>,
}

impl DemoEntry {
    /// Back to "not parsed": no result in memory, nothing derived in the meta.
    fn reset(&mut self) {
        self.parsed = None;
        self.meta.status = DemoStatus::New;
        self.meta.error = None;
        self.meta.summary = None;
        self.meta.map_name = None;
        self.meta.parsed_at = None;
    }
}

struct ParseRequest {
    id: String,
    automatic: bool,
}

#[derive(Default)]
struct ParseQueue {
    pending: VecDeque<ParseRequest>,
    active: HashSet<String>,
    workers: usize,
}

impl ParseQueue {
    fn contains(&self, id: &str) -> bool {
        self.active.contains(id) || self.pending.iter().any(|job| job.id == id)
    }
    fn is_empty(&self) -> bool {
        self.active.is_empty() && self.pending.is_empty()
    }
}

struct ReplayRequest {
    id: String,
    replies: Vec<std::sync::mpsc::Sender<std::result::Result<PathBuf, String>>>,
}

#[derive(Default)]
struct ReplayQueue {
    pending: VecDeque<ReplayRequest>,
    active: Option<ReplayRequest>,
    running: bool,
}

pub struct Engine {
    store: Store,
    data_dir: PathBuf,
    notify: Arc<dyn Notify>,
    demos: Mutex<HashMap<String, DemoEntry>>,
    registered_demos: Mutex<Vec<PathBuf>>,
    parsing: Mutex<ParseQueue>,
    parser: Arc<DemoParser>,
    render_queue: Mutex<VecDeque<String>>,
    /// Serializes short state/cache mutations, never replay computation.
    data_lock: Mutex<()>,
    replay_queue: Mutex<ReplayQueue>,
    /// Serializes radar extraction (one Source2Viewer-CLI at a time).
    radar_lock: Mutex<()>,
    // ponytail: one scoring worker; use per-demo locks if concurrent scoring becomes necessary.
    scoring_lock: Mutex<()>,
    analysis_queue: Mutex<crate::scoring::queue::Queue>,
    active_job: Mutex<Option<(String, Arc<AtomicBool>)>>,
    render_worker_running: AtomicBool,
    render_paused: AtomicBool,
    shutting_down: AtomicBool,
    setup: Mutex<HashMap<SetupTool, SetupState>>,
    tool_checks: Mutex<HashMap<SetupTool, crate::render::diagnostics::ToolCheck>>,
    tool_check_lock: Mutex<()>,
    settings_lock: Mutex<()>,
    tool_access: Arc<tool_access::ToolAccess>,
    media_worker: clip_media::MediaWorker,
    job_reservations: Mutex<HashMap<String, usize>>,
}

/// Keeps a specific owned job's files available while another task consumes them.
pub struct JobReservation {
    engine: std::sync::Weak<Engine>,
    id: String,
}
impl Drop for JobReservation {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.upgrade() {
            if let Ok(mut reservations) = engine.job_reservations.lock() {
                if let Some(count) = reservations.get_mut(&self.id) {
                    *count -= 1;
                    if *count == 0 {
                        reservations.remove(&self.id);
                    }
                }
            }
        }
    }
}

impl Engine {
    pub fn new(data_dir: PathBuf, notify: Arc<dyn Notify>) -> Result<Arc<Self>> {
        let store = Store::open(data_dir.clone())?;
        if let Err(error) = auto_attempts::migrate(&data_dir, &store.list_jobs()) {
            // A damaged automatic ledger must never prevent playback/manual exports.
            eprintln!("Automatic exports are paused until their history can be read: {error:#}");
        }
        // Analysis jobs belong only to this process; remove the former on-disk queue.
        if let Err(error) = std::fs::remove_file(data_dir.join("analysis-jobs.json")) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("cannot remove old analysis queue: {error}");
            }
        }

        // Queued work survives a restart. Interrupted recordings require an explicit retry;
        // never silently replace an existing export with a new attempt.
        let mut recovered_queue = VecDeque::new();
        for mut job in store.list_jobs() {
            if job.status == JobStatus::Queued {
                job.stage = Some("recovered".into());
                store.save_job(&job)?;
                recovered_queue.push_back(job.id.clone());
            } else if job.status == JobStatus::Running {
                job.status = JobStatus::Error;
                job.error = Some("Recording was interrupted. Existing files were kept; retry this job when ready.".into());
                job.error_code = Some(crate::ErrorCode::AppClosed);
                job.finished_at = Some(now());
                let _ = store.save_job(&job);
            }
        }
        let registered_demos = store.registered_demos()?;
        let engine = Arc::new(Self {
            store,
            data_dir: data_dir.clone(),
            notify,
            demos: Mutex::new(HashMap::new()),
            registered_demos: Mutex::new(registered_demos),
            parsing: Mutex::new(ParseQueue::default()),
            parser: Arc::new(DemoParser::new()),
            render_queue: Mutex::new(recovered_queue),
            data_lock: Mutex::new(()),
            replay_queue: Mutex::new(ReplayQueue::default()),
            radar_lock: Mutex::new(()),
            scoring_lock: Mutex::new(()),
            analysis_queue: Mutex::new(crate::scoring::queue::Queue::default()),
            active_job: Mutex::new(None),
            render_worker_running: AtomicBool::new(false),
            render_paused: AtomicBool::new(data_dir.join("queue-paused").exists()),
            shutting_down: AtomicBool::new(false),
            setup: Mutex::new(HashMap::new()),
            tool_checks: Mutex::new(HashMap::new()),
            tool_check_lock: Mutex::new(()),
            settings_lock: Mutex::new(()),
            tool_access: Arc::new(tool_access::ToolAccess::default()),
            media_worker: clip_media::MediaWorker::default(),
            job_reservations: Mutex::new(HashMap::new()),
        });
        crate::render::setup::recover_installs(
            &engine.tools_dir(),
            &engine.overrides(&engine.store.settings()),
        );
        // This fork never installs or removes plugins in the user's game directory.
        for tool in [SetupTool::Hlae, SetupTool::Ffmpeg, SetupTool::Vrf] {
            let journal = engine.tools_dir().join(format!(".install-{tool:?}.json"));
            let workspace = std::fs::read(&journal)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<PathBuf>(&bytes).ok())
                .unwrap_or_else(|| engine.tools_dir().join(format!(".install-{tool:?}")));
            if let Some(dir) = crate::render::setup::pending_directory(&workspace) {
                if let Some(parent) = dir.parent() {
                    let directory = (parent != engine.tools_dir()).then(|| parent.to_path_buf());
                    engine.start_setup(tool, true, directory);
                }
            }
        }
        Ok(engine)
    }

    pub fn queue_paused(&self) -> bool {
        self.render_paused.load(Ordering::SeqCst)
    }

    /// Pause between clips/jobs. The current owned recording finishes or can be cancelled.
    pub fn set_queue_paused(self: &Arc<Self>, paused: bool) -> Result<()> {
        let marker = self.data_dir.join("queue-paused");
        if paused {
            std::fs::write(&marker, b"paused")?;
        } else if marker.exists() {
            std::fs::remove_file(&marker)?;
        }
        self.render_paused.store(paused, Ordering::SeqCst);
        self.pump();
        Ok(())
    }

    pub fn resume_saved_queue(self: &Arc<Self>) {
        self.pump();
    }

    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        self.media_worker.cancel_all();
        if let Some((_, cancel)) = self.active_job.lock().unwrap().as_ref() {
            cancel.store(true, Ordering::SeqCst);
        }
    }

    pub fn analysis_jobs(&self) -> Vec<crate::scoring::queue::Job> {
        self.analysis_queue
            .lock()
            .unwrap()
            .jobs
            .iter()
            .rev()
            .cloned()
            .collect()
    }

    pub fn enqueue_analysis(
        self: &Arc<Self>,
        id: &str,
        force: bool,
    ) -> Result<crate::scoring::queue::Job> {
        let _guard = self.data_lock.lock().unwrap();
        let (meta, parsed) = self.get_demo(id).ok_or_else(|| anyhow!("demo not found"))?;
        anyhow::ensure!(parsed.is_some(), "demo not parsed");
        let mut queue = self.analysis_queue.lock().unwrap();
        let job = queue.enqueue(&meta.id, force);
        self.notify
            .notify(Event::AnalysisJobChanged { job: job.clone() });
        if !queue.worker_running {
            queue.worker_running = true;
            let engine = self.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("analysis-worker".into())
                .spawn(move || engine.run_analysis_queue())
            {
                queue.worker_running = false;
                for entry in queue
                    .jobs
                    .iter_mut()
                    .filter(|j| j.status == crate::scoring::queue::Status::Queued)
                {
                    entry.status = crate::scoring::queue::Status::Error;
                    entry.error = Some(format!("Cannot start analysis worker: {error}"));
                    entry.finished_at = Some(now());
                    entry.revision += 1;
                    self.notify
                        .notify(Event::AnalysisJobChanged { job: entry.clone() });
                }
                return Err(error.into());
            }
        }
        Ok(job)
    }

    fn analysis_progress(&self, demo_id: &str, step: u8) {
        let mut queue = self.analysis_queue.lock().unwrap();
        if let Some(job) = queue
            .jobs
            .iter_mut()
            .find(|j| j.demo_id == demo_id && j.status == crate::scoring::queue::Status::Running)
        {
            job.step = Some(step);
            job.phase = Some(match step {
                2 => crate::scoring::queue::Phase::Analyzing,
                3 => crate::scoring::queue::Phase::Saving,
                _ => crate::scoring::queue::Phase::Preparing,
            });
            job.revision += 1;
            let job = job.clone();
            self.notify.notify(Event::AnalysisJobChanged { job });
        }
    }

    fn analysis_phase(&self, demo_id: &str, phase: crate::scoring::queue::Phase) {
        let mut queue = self.analysis_queue.lock().unwrap();
        if let Some(job) = queue.jobs.iter_mut().find(|job| {
            job.demo_id == demo_id && job.status == crate::scoring::queue::Status::Running
        }) {
            if job.phase.as_ref() != Some(&phase) {
                job.phase = Some(phase);
                job.revision += 1;
                self.notify
                    .notify(Event::AnalysisJobChanged { job: job.clone() });
            }
        }
    }

    #[cfg(windows)]
    fn scoring_compute_lease(&self, demo_id: &str) -> Result<crate::render::lease::ComputeLease> {
        loop {
            anyhow::ensure!(
                !self.shutting_down.load(Ordering::SeqCst),
                "Analysis stopped because ClipForge is closing."
            );
            if let Some(lease) = crate::render::lease::ComputeLease::try_acquire()? {
                self.analysis_phase(demo_id, crate::scoring::queue::Phase::Preparing);
                return Ok(lease);
            }
            self.analysis_phase(demo_id, crate::scoring::queue::Phase::WaitingForCompute);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    fn run_analysis_queue(self: Arc<Self>) {
        loop {
            let _tools = self.tool_access.use_tools(&[SetupTool::Vrf]);
            let job = {
                let mut queue = self.analysis_queue.lock().unwrap();
                let Some(job) = queue.claim() else {
                    queue.worker_running = false;
                    return;
                };
                self.notify
                    .notify(Event::AnalysisJobChanged { job: job.clone() });
                job
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.score_match_unlocked(&job.demo_id, job.force)
            }))
            .unwrap_or_else(|_| {
                Err(anyhow!(
                    "Analysis worker stopped unexpectedly. Retry the analysis."
                ))
            });
            // A panic must not poison later independent jobs.
            self.scoring_lock.clear_poison();
            let mut queue = self.analysis_queue.lock().unwrap();
            let stored = queue.jobs.iter_mut().find(|j| j.id == job.id).unwrap();
            stored.finished_at = Some(now());
            stored.phase = None;
            stored.revision += 1;
            match result {
                Ok(_) => {
                    stored.status = crate::scoring::queue::Status::Done;
                    stored.error = None;
                }
                Err(error) => {
                    stored.status = crate::scoring::queue::Status::Error;
                    stored.error = Some(format!("{error:#}"));
                }
            }
            let job = queue.jobs.iter().find(|j| j.id == job.id).unwrap().clone();
            self.notify.notify(Event::AnalysisJobChanged { job });
        }
    }

    /// History remains readable even if its source demo is missing or has changed.
    pub fn scoring_history(
        &self,
        id: &str,
        player_id: &str,
    ) -> Result<Vec<crate::scoring::Assessment>> {
        crate::scoring::history::list(&self.data_dir, id, player_id)
    }

    pub fn scoring_match_history(
        &self,
        id: &str,
    ) -> Result<std::collections::BTreeMap<String, Vec<crate::scoring::Assessment>>> {
        crate::scoring::history::list_match(&self.data_dir, id)
    }

    /// Explicit scoring only. Basic demo auto-analysis never enters this path.
    pub fn score_match(
        &self,
        id: &str,
        force: bool,
    ) -> Result<crate::scoring::history::MatchResponse> {
        let _tools = self.tool_access.use_tools(&[SetupTool::Vrf]);
        self.score_match_unlocked(id, force)
    }

    fn score_match_unlocked(
        &self,
        id: &str,
        force: bool,
    ) -> Result<crate::scoring::history::MatchResponse> {
        use crate::scoring::{self, Assessment};
        use std::io::Read;
        use std::time::Instant;
        let _guard = self
            .scoring_lock
            .lock()
            .map_err(|_| anyhow!("analysis worker lock failed"))?;
        let progress = |step| {
            self.analysis_progress(id, step);
            self.notify.notify(Event::ScoringProgress {
                id: id.into(),
                step,
            });
        };
        progress(1);
        let started = Instant::now();
        let (meta, parsed) = self.get_demo(id).ok_or_else(|| anyhow!("demo not found"))?;
        let parsed = parsed.ok_or_else(|| anyhow!("demo not parsed"))?;
        let players: Vec<_> = parsed
            .info
            .players
            .iter()
            .map(|p| p.steamid.clone())
            .collect();
        anyhow::ensure!(!players.is_empty(), "demo has no players");
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1);
        }
        let mut source_file = options.open(&meta.path)?;
        let metadata = source_file.metadata()?;
        let modified = metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis() as f64;
        anyhow::ensure!(
            meta.same_file(metadata.len(), modified),
            "demo changed; parse it again before scoring"
        );
        anyhow::ensure!(
            crate::demo_readiness::is_complete(&mut source_file)?,
            "incomplete scoring source"
        );
        use std::io::Seek;
        source_file.rewind()?;
        let mut source_hash = sha1_smol::Sha1::new();
        let mut source_buffer = [0; 65536];
        loop {
            let count = source_file.read(&mut source_buffer)?;
            if count == 0 {
                break;
            }
            source_hash.update(&source_buffer[..count]);
        }
        let fingerprint = format!("sha1:{}", source_hash.digest());
        crate::scoring::history::track_source(&self.data_dir, id, &fingerprint)?;
        if let Err(error) = scoring::history::clear_intermediates(&self.data_dir, &fingerprint) {
            eprintln!("could not clean old analysis intermediates for {id}: {error:#}");
        }
        let mut histories = self.scoring_match_history(id)?;
        histories.retain(|player, _| players.contains(player));
        for player in &players {
            histories.entry(player.clone()).or_default();
        }
        let tools = self.tool_paths();
        let game = tools.cs2_dir.ok_or_else(|| anyhow!("CS2 folder not set"))?;
        let game_content_fingerprint =
            crate::analysis::native_body::game_content_fingerprint(&game)?;
        if !force
            && histories.values().all(|records| {
                records.iter().any(|r| {
                    r.demo_fingerprint == fingerprint
                        && r.ruleset_version == scoring::RULESET_VERSION
                        && r.input_provenance["native"]["gameContentFingerprint"].as_str()
                            == Some(game_content_fingerprint.as_str())
                })
            })
        {
            let provenance = &histories
                .values()
                .flat_map(|records| records.iter())
                .find(|r| {
                    r.demo_fingerprint == fingerprint
                        && r.ruleset_version == scoring::RULESET_VERSION
                        && r.input_provenance["native"]["gameContentFingerprint"].as_str()
                            == Some(game_content_fingerprint.as_str())
                })
                .expect("all players have matching history")
                .input_provenance;
            let generic_bytes = provenance["shared"]["bytes"].as_u64().unwrap_or(0);
            let diagnostic_bytes = provenance["diagnosticBytes"].as_u64().unwrap_or(0);
            return Ok(scoring::history::MatchResponse {
                source_fingerprint: fingerprint,
                players: histories,
                preparation_seconds: started.elapsed().as_secs_f64(),
                analysis_seconds: 0.0,
                shared_bytes: generic_bytes + diagnostic_bytes,
                generic_bytes,
                diagnostic_bytes,
            });
        }
        // Cached reads above stay responsive. Only full uncached analysis shares
        // the recording/AI lease, before allocating the complete source bytes.
        #[cfg(windows)]
        let _compute_lease = self.scoring_compute_lease(id)?;
        source_file.rewind()?;
        let mut bytes = Vec::new();
        source_file.read_to_end(&mut bytes)?;
        let mut shared_file = crate::analysis::compact::prepare(
            &self.parser,
            &bytes,
            &self.data_dir,
            crate::analysis::compact::memory_limit(),
        )?;
        drop(bytes);
        let shared_storage = if shared_file.is_rolled() {
            "temporaryFile"
        } else {
            "memory"
        };
        let mut shared_hash = sha1_smol::Sha1::new();
        let mut shared_bytes = 0;
        let mut buffer = [0; 65536];
        loop {
            let n = shared_file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            shared_hash.update(&buffer[..n]);
            shared_bytes += n as u64;
        }
        shared_file.rewind()?;
        let vrf = tools
            .vrf_exe
            .ok_or_else(|| anyhow!("Source 2 Viewer CLI not installed"))?;
        let native = crate::analysis::native_body::prepare_reader(
            &mut shared_file,
            &self.data_dir,
            &game,
            &vrf,
        )?;
        shared_file.rewind()?;
        let visibility_result = crate::analysis::visibility_assets::prepare(
            &self.data_dir,
            &game,
            &vrf,
            &parsed.info.map_name,
        );
        let visibility_error = visibility_result.as_ref().err().map(|e| format!("{e:#}"));
        let visibility = visibility_result.ok();
        let header = &native.header;
        anyhow::ensure!(
            header.source.demo_fingerprint.as_deref() == Some(&fingerprint),
            "shared source mismatch"
        );
        let preparation_seconds = started.elapsed().as_secs_f64();
        progress(2);
        let started = Instant::now();
        let (mut checks, body_coverage) = scoring::native::evaluate(
            &mut shared_file,
            &native,
            &fingerprint,
            &players,
            &parsed.rounds,
            &parsed.kills,
            visibility.as_ref(),
        )?;
        drop(shared_file);
        let provenance = serde_json::json!({
            "shared":{"contract":header.contract,"source":header.source,"availability":header.data,
                "coverage":native.coverage,"bytes":shared_bytes,"contentFingerprint":format!("sha1:{}", shared_hash.digest()),
                "storage":shared_storage,"retained":false},
            "visibilityError":visibility_error,
            "visibilityAssets":visibility.as_ref().map(|p|serde_json::json!({"bytes":p.bytes,"fingerprint":p.fingerprint})),
            "diagnosticBytes":0,
            "statistics":{"mode":"per-rule-occurrences","deduplication":"same-rule-round-target-overlap","crossRuleCounts":"independent"},
            "native":{
                "producer":"native-animgraph2-5", "coverage":body_coverage,
                "gameContentFingerprint":game_content_fingerprint,
                "resourceContentId":native.assets.resource_content_id,
                "sharedAssetBytes":native.assets.total_bytes,"clientSha256":native.client_sha256,
                "customTasksVerified":native.custom_tasks_verified,
                "assetPrecision":"VRF DATA/MDAT text; tested maximum raw clip component error 8.35e-7; network state lossless",
                "qualification":"partial task reconstruction; historical asset compatibility and independent rule calibration unqualified",
                "unknownObstruction":true
            }
        });
        progress(3);
        let mut records = Vec::new();
        let created_at = crate::store::now();
        for player_id in &players {
            let mut checks = checks
                .remove(player_id)
                .ok_or_else(|| anyhow!("missing player checks"))?;
            let state = scoring::statistics::summarize(&mut checks);
            records.push(Assessment {
                schema_version: 2,
                id: String::new(),
                created_at: created_at.clone(),
                demo_id: id.into(),
                demo_fingerprint: fingerprint.clone(),
                player_id: player_id.clone(),
                tick_rate: parsed.info.tick_rate,
                ruleset_version: scoring::RULESET_VERSION.into(),
                checks,
                input_provenance: provenance.clone(),
                state,
            });
        }
        scoring::history::save_match(&self.data_dir, &mut records)?;
        for record in records {
            histories.insert(record.player_id.clone(), vec![record]);
        }
        Ok(scoring::history::MatchResponse {
            source_fingerprint: fingerprint,
            players: histories,
            preparation_seconds,
            analysis_seconds: started.elapsed().as_secs_f64(),
            shared_bytes,
            generic_bytes: shared_bytes,
            diagnostic_bytes: 0,
        })
    }

    // ---- paths / settings ----
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
    pub fn settings(&self) -> Settings {
        self.store.settings()
    }
    pub fn recording_defaults(&self) -> RenderOptions {
        self.store.settings().render_defaults
    }
    /// Validate, then write. Returns the problems instead of writing when there are any.
    pub fn save_settings(&self, settings: Settings) -> Result<(), Vec<String>> {
        let _guard = self.settings_lock.lock().unwrap();
        let clean = settings.normalized();
        let problems = self.validate_settings(&clean);
        if !problems.is_empty() {
            return Err(problems);
        }
        self.store
            .save_settings(&clean)
            .map_err(|e| vec![format!("{e:#}")])?;
        self.clean_leftovers();
        Ok(())
    }
    /// Sizes of the disposable folders: (parsed, clips, radar).
    pub fn storage_bytes(&self) -> (u64, u64, u64, u64) {
        (
            self.store.parsed_bytes(),
            self.store.clips_bytes(),
            self.store.radar_bytes(),
            self.store.anomaly_bytes(),
        )
    }
    pub fn clear_anomaly_data(&self) -> Result<u64> {
        let _guard = self
            .scoring_lock
            .try_lock()
            .map_err(|_| anyhow!("analysis is running"))?;
        let queue = self.analysis_queue.lock().unwrap();
        anyhow::ensure!(
            !queue.jobs.iter().any(|job| matches!(
                job.status,
                crate::scoring::queue::Status::Queued | crate::scoring::queue::Status::Running
            )),
            "analysis is queued or running"
        );
        self.store.clear_anomaly_data()
    }
    pub fn clear_radar(&self) -> u64 {
        self.store.clear_radar()
    }
    pub fn list_jobs(&self) -> Result<Vec<RenderJob>> {
        let mut jobs = self.store.list_jobs();
        for job in &mut jobs {
            let mut available = Vec::with_capacity(job.outputs.len());
            for output in std::mem::take(&mut job.outputs) {
                match Path::new(&output.file).try_exists() {
                    Ok(true) => available.push(output),
                    Ok(false) => {}
                    Err(error) => {
                        return Err(anyhow!("Cannot check video {}: {error}", output.file))
                    }
                }
            }
            job.outputs = available;
        }
        Ok(jobs)
    }
    pub fn tools_dir(&self) -> PathBuf {
        self.data_dir.join("tools")
    }
    pub fn overrides(&self, s: &Settings) -> PathOverrides {
        let p = |v: &Option<String>| v.as_ref().map(PathBuf::from);
        PathOverrides {
            steam_dir: p(&s.steam_dir),
            cs2_dir: p(&s.cs2_dir),
            hlae_exe: p(&s.hlae_exe),
            ffmpeg_exe: p(&s.ffmpeg_exe),
            vrf_exe: p(&s.vrf_exe),
        }
    }
    pub fn tool_paths(&self) -> ToolPaths {
        resolve_tool_paths(&self.tools_dir(), &self.overrides(&self.store.settings()))
    }
    pub fn doctor(&self) -> DoctorReport {
        doctor(&self.tools_dir(), &self.overrides(&self.store.settings()))
    }
    /// Undo an interrupted run's plugin install — skipped while a render is active.
    pub fn clean_leftovers(&self) -> bool {
        // Legacy upstream migration changed game files. This local fork owns J: files only.
        false
    }
    pub fn detected(&self) -> Detected {
        let steam_dir = find_steam_dir();
        let cs2_dir = find_cs2_dir(steam_dir.as_deref());
        Detected {
            replays_dir: cs2_dir.as_deref().map(replays_dir),
            steam_dir,
            cs2_dir,
        }
    }
    pub fn replay_folders(&self) -> Vec<PathBuf> {
        let s = self.store.settings();
        let mut folders: Vec<PathBuf> = vec![];
        if s.scan_game_replays {
            let cs2 = resolve_tool_paths(&self.tools_dir(), &self.overrides(&s)).cs2_dir;
            if let Some(cs2) = cs2 {
                folders.push(replays_dir(&cs2));
            }
        }
        folders.extend(s.replay_folders.iter().map(PathBuf::from));
        folders.dedup();
        folders.into_iter().filter(|f| f.is_dir()).collect()
    }

    pub fn validate_settings(&self, s: &Settings) -> Vec<String> {
        let mut problems = vec![];
        if let Err(error) = s.render_defaults.validate() {
            problems.push(error.to_string());
        }
        let o = self.overrides(s);
        if let Some(steam) = &o.steam_dir {
            if !steam.join("steam.exe").is_file() {
                problems.push(format!("steam.exe not found under {}", steam.display()));
            }
        }
        if let Some(cs2) = &o.cs2_dir {
            if !crate::render::paths::cs2_exe_in(cs2).is_file() {
                problems.push(format!(
                    "game\\bin\\win64\\cs2.exe not found under {}",
                    cs2.display()
                ));
            }
        }
        for path in [&o.hlae_exe, &o.ffmpeg_exe, &o.vrf_exe]
            .into_iter()
            .flatten()
        {
            let directory = crate::render::paths::installation_directory(path);
            if !directory.is_absolute()
                || directory
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                problems.push(format!(
                    "Choose an absolute tool directory: {}",
                    directory.display()
                ));
            }
        }
        for f in &s.replay_folders {
            if !Path::new(f).is_dir() {
                problems.push(format!("folder not found: {f}"));
            }
        }
        problems
    }

    // ---- demos ----
    /// Stable id for a path (case-insensitive, so D:\x and d:\x match).
    pub fn demo_id(path: &Path) -> String {
        let key = path.to_string_lossy().to_lowercase().replace('/', "\\");
        sha1_smol::Sha1::from(key.as_bytes()).digest().to_string()[..12].to_string()
    }

    fn is_dem(path: &Path) -> bool {
        path.extension()
            .map(|e| e.to_string_lossy().eq_ignore_ascii_case("dem"))
            .unwrap_or(false)
    }

    /// Scan configured folders plus individually registered files in their original locations.
    fn demo_paths(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self
            .registered_demos
            .lock()
            .unwrap()
            .iter()
            .filter(|path| Self::is_dem(path) && path.is_file())
            .cloned()
            .collect();
        for folder in self.replay_folders() {
            let Ok(rd) = std::fs::read_dir(&folder) else {
                continue;
            };
            for entry in rd.flatten() {
                let path = entry.path();
                if Self::is_dem(&path) && path.is_file() {
                    out.push(path);
                }
            }
        }
        let mut seen = HashSet::new();
        out.retain(|p| seen.insert(Self::demo_id(p)));
        out
    }

    /// Rescan the file system and merge with the in-memory parse state.
    pub fn list_demos(self: &Arc<Self>) -> Vec<DemoMeta> {
        // Everything that touches the disk happens before the lock.
        let scanned: Vec<(PathBuf, u64, f64, f64, Option<f64>)> = self
            .demo_paths()
            .into_iter()
            .filter_map(|path| {
                let st = std::fs::metadata(&path).ok()?;
                let mtime_ms = st
                    .modified()
                    .ok()
                    .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as f64)
                    .unwrap_or(0.0);
                let created_ms = st
                    .created()
                    .ok()
                    .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as f64)
                    .unwrap_or(mtime_ms);
                let match_time_ms = crate::store::match_time_ms(&path);
                Some((path, st.len(), mtime_ms, created_ms, match_time_ms))
            })
            .collect();
        let keep: HashSet<String> = scanned
            .iter()
            .map(|(p, _, _, _, _)| Self::demo_id(p))
            .collect();
        let list = {
            let mut demos = self.demos.lock().unwrap();
            let mut stale: Vec<String> = vec![];
            for (path, bytes, mtime_ms, created_ms, match_time_ms) in scanned {
                let id = Self::demo_id(&path);
                let entry = demos.entry(id.clone()).or_insert_with(|| {
                    // A result stored by an earlier session counts as parsed (loaded lazily).
                    let failure = self.store.parse_error(&id);
                    let stored = self.store.read_summary(&id, bytes, mtime_ms);
                    DemoEntry {
                        meta: DemoMeta {
                            id,
                            name: path
                                .file_name()
                                .map(|f| f.to_string_lossy().to_string())
                                .unwrap_or_default(),
                            path: path.to_string_lossy().to_string(),
                            bytes,
                            mtime_ms,
                            created_ms,
                            match_time_ms,
                            status: if failure.is_some() {
                                DemoStatus::Error
                            } else if stored.is_some() {
                                DemoStatus::Parsed
                            } else {
                                DemoStatus::New
                            },
                            error: failure,
                            map_name: stored.as_ref().map(|s| s.map_name.clone()),
                            parsed_at: stored.as_ref().map(|s| s.parsed_at.clone()),
                            summary: stored.map(|s| s.summary),
                        },
                        parsed: None,
                        auto_complete: None,
                    }
                });
                entry.meta.path = path.to_string_lossy().to_string();
                if entry.meta.status == DemoStatus::New || !entry.meta.same_file(bytes, mtime_ms) {
                    entry.auto_complete = None;
                }
                // File changed under us: the parse result no longer describes it.
                if entry.meta.status == DemoStatus::Parsed && !entry.meta.same_file(bytes, mtime_ms)
                {
                    entry.reset();
                    stale.push(entry.meta.id.clone());
                }
                entry.meta.bytes = bytes;
                entry.meta.mtime_ms = mtime_ms;
                entry.meta.created_ms = created_ms;
                entry.meta.match_time_ms = match_time_ms;
            }
            let parsing = self.parsing.lock().unwrap();
            demos.retain(|id, _| keep.contains(id) || parsing.contains(id));
            for id in &stale {
                let _ = self.store.delete_parsed(id);
            }
            let mut list: Vec<DemoMeta> = demos.values().map(|e| e.meta.clone()).collect();
            list.sort_by(|a, b| {
                b.date_ms()
                    .total_cmp(&a.date_ms())
                    .then_with(|| a.id.cmp(&b.id))
            });
            list
        };
        self.store.prune_parsed(&keep);
        self.enqueue_unparsed();
        let demos = self.demos.lock().unwrap();
        list.into_iter()
            .map(|meta| {
                demos
                    .get(&meta.id)
                    .map(|entry| entry.meta.clone())
                    .unwrap_or(meta)
            })
            .collect()
    }

    /// Meta + parse result; a result stored on disk is loaded on first use.
    pub fn get_demo(&self, id: &str) -> Option<(DemoMeta, Option<Arc<ParsedDemo>>)> {
        let (meta, parsed) = self
            .demos
            .lock()
            .unwrap()
            .get(id)
            .map(|e| (e.meta.clone(), e.parsed.clone()))?;
        if parsed.is_some() || meta.status != DemoStatus::Parsed {
            return Some((meta, parsed));
        }
        match self.store.read_parsed(id).map(Arc::new) {
            Some(p) => {
                let mut demos = self.demos.lock().unwrap();
                if let Some(e) = demos.get_mut(id) {
                    // A cache read begun before clearing must not restore the old result.
                    if e.meta.status != DemoStatus::Parsed || e.meta.parsed_at != meta.parsed_at {
                        return Some((e.meta.clone(), e.parsed.clone()));
                    }
                    if e.parsed.is_none() {
                        e.parsed = Some(p.clone());
                    }
                }
                Some((meta, Some(p)))
            }
            None => {
                // Stored result unreadable: fall back to "not parsed".
                let _ = self.store.delete_parsed(id);
                self.update_meta(id, |e| {
                    if e.meta.status != DemoStatus::Error {
                        e.reset();
                    }
                })
                .map(|m| (m, None))
            }
        }
    }
    pub fn parsed(&self, id: &str) -> Option<Arc<ParsedDemo>> {
        self.get_demo(id).and_then(|(_, p)| p)
    }

    /// Register the selected file in place. Only derived data belongs to the store.
    pub fn add_demo(self: &Arc<Self>, path: &Path) -> Result<DemoMeta> {
        if !path.is_file() || !Self::is_dem(path) {
            return Err(anyhow!("not a .dem file: {}", path.display()));
        }
        let path = std::path::absolute(path)?;
        let id = Self::demo_id(&path);
        {
            let mut registered = self.registered_demos.lock().unwrap();
            if !registered.iter().any(|p| Self::demo_id(p) == id) {
                let mut updated = registered.clone();
                updated.push(path);
                self.store.save_registered_demos(&updated)?;
                *registered = updated;
            }
        }
        self.list_demos()
            .into_iter()
            .find(|m| m.id == id)
            .ok_or_else(|| anyhow!("demo no longer available after registering"))
    }

    pub fn is_parsing(&self, id: &str) -> bool {
        self.parsing.lock().unwrap().contains(id)
    }

    fn update_meta(&self, id: &str, f: impl FnOnce(&mut DemoEntry)) -> Option<DemoMeta> {
        let mut demos = self.demos.lock().unwrap();
        let entry = demos.get_mut(id)?;
        f(entry);
        Some(entry.meta.clone())
    }

    fn enqueue_unparsed(self: &Arc<Self>) {
        let mut candidates: Vec<_> = self
            .demos
            .lock()
            .unwrap()
            .values()
            .filter(|e| e.meta.status == DemoStatus::New && e.auto_complete != Some(false))
            .map(|e| (e.meta.id.clone(), e.meta.mtime_ms))
            .collect();
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (id, _) in candidates {
            if let Err(error) = self.enqueue_parse(&id, true) {
                eprintln!("could not enqueue {id}: {error:#}");
            }
        }
    }

    pub fn parse_demo(self: &Arc<Self>, id: &str) -> Result<()> {
        self.enqueue_parse(id, false)
    }

    fn enqueue_parse(self: &Arc<Self>, id: &str, automatic: bool) -> Result<()> {
        {
            let _guard = self.data_lock.lock().unwrap();
            let mut demos = self.demos.lock().unwrap();
            let entry = demos.get_mut(id).ok_or_else(|| anyhow!("demo not found"))?;
            let mut queue = self.parsing.lock().unwrap();
            if queue.contains(id)
                || (automatic
                    && (entry.meta.status != DemoStatus::New || entry.auto_complete == Some(false)))
            {
                return Ok(());
            }
            entry.reset();
            entry.meta.status = DemoStatus::Queued;
            queue.pending.push_back(ParseRequest {
                id: id.to_owned(),
                automatic,
            });
            // Publish Queued before a worker can publish Validating or Parsing.
            self.notify.notify(Event::DemoChanged {
                demo: entry.meta.clone(),
            });
        }
        self.start_parse_workers();
        Ok(())
    }

    fn start_parse_workers(self: &Arc<Self>) {
        let mut queue = self.parsing.lock().unwrap();
        let count = queue.pending.len().min(PARSE_CONCURRENCY - queue.workers);
        let mut failed = Vec::new();
        for _ in 0..count {
            queue.workers += 1;
            let engine = self.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("demo-parser".into())
                .spawn(move || engine.run_parse_queue())
            {
                queue.workers -= 1;
                if let Some(job) = queue.pending.pop_front() {
                    queue.active.insert(job.id.clone());
                    failed.push((job.id, error));
                }
            }
        }
        drop(queue);
        for (id, error) in failed {
            self.finish_parse_error(&id, format!("could not start the parse thread: {error}"));
            self.parsing.lock().unwrap().active.remove(&id);
        }
    }

    fn run_parse_queue(self: &Arc<Self>) {
        loop {
            let (job, meta) = {
                let _guard = self.data_lock.lock().unwrap();
                let mut demos = self.demos.lock().unwrap();
                let mut queue = self.parsing.lock().unwrap();
                let Some(job) = queue.pending.pop_front() else {
                    queue.workers -= 1;
                    return;
                };
                let Some(entry) = demos.get_mut(&job.id) else {
                    continue;
                };
                queue.active.insert(job.id.clone());
                entry.meta.status = if job.automatic {
                    DemoStatus::Validating
                } else {
                    DemoStatus::Parsing
                };
                (job, entry.meta.clone())
            };
            self.notify
                .notify(Event::DemoChanged { demo: meta.clone() });
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.parse_source(&job.id, &meta, job.automatic)
            }));
            let outcome = match result {
                Ok(Ok(Some(parsed))) => self
                    .store
                    .write_parsed(&job.id, &meta, &parsed)
                    .and_then(|_| self.store.clear_parse_error(&job.id))
                    .map(|_| Some(parsed))
                    .map_err(|e| format!("Cannot save parse result: {e:#}")),
                Ok(Ok(None)) => Ok(None),
                Ok(Err(error)) => Err(format!("{error:#}")),
                Err(_) => Err("parser crashed (unsupported or corrupt demo?)".into()),
            };
            match outcome {
                Ok(parsed) => {
                    let fresh = self.update_meta(&job.id, |entry| {
                        if let Some(parsed) = parsed {
                            entry.meta.status = DemoStatus::Parsed;
                            entry.meta.error = None;
                            entry.meta.map_name = Some(parsed.info.map_name.clone());
                            entry.meta.parsed_at = Some(parsed.parsed_at.clone());
                            entry.meta.summary = Some(DemoSummary::of(&parsed));
                            entry.parsed = if job.automatic {
                                None
                            } else {
                                Some(Arc::new(parsed))
                            };
                        } else {
                            entry.meta.status = DemoStatus::New;
                            entry.auto_complete = Some(false);
                        }
                    });
                    if let Some(demo) = fresh {
                        self.notify.notify(Event::DemoChanged { demo });
                    }
                }
                Err(error) => self.finish_parse_error(&job.id, error),
            }
            self.parsing.lock().unwrap().active.remove(&job.id);
        }
    }

    fn parse_source(
        &self,
        id: &str,
        meta: &DemoMeta,
        automatic: bool,
    ) -> Result<Option<ParsedDemo>> {
        use std::io::Read;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        if automatic {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1); // Refuse active writers throughout validation and parsing.
        }
        let mut source = match options.open(&meta.path) {
            Ok(file) => file,
            Err(_) if automatic => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if automatic {
            let metadata = source.metadata()?;
            let modified = metadata
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis() as f64;
            if !meta.same_file(metadata.len(), modified) {
                return Ok(None);
            }
        }
        let mut bytes = Vec::new();
        source.read_to_end(&mut bytes)?;
        if automatic && !crate::demo_readiness::is_complete_bytes(&bytes)? {
            return Ok(None);
        }
        {
            let _guard = self.data_lock.lock().unwrap();
            self.store.delete_parsed(id)?;
            self.store
                .write_parse_error(id, "Parsing was interrupted; parse manually to retry.")?;
        }
        if automatic {
            if let Some(demo) = self.update_meta(id, |e| e.meta.status = DemoStatus::Parsing) {
                self.notify.notify(Event::DemoChanged { demo });
            }
        }
        self.parser
            .load_demo_bytes(Path::new(&meta.path), &bytes)
            .map(build_parsed_demo)
            .map(Some)
    }

    fn finish_parse_error(&self, id: &str, message: String) {
        if let Err(error) = self.store.write_parse_error(id, &message) {
            eprintln!("could not store parse failure for {id}: {error:#}");
        }
        let fresh = self.update_meta(id, |e| {
            e.reset();
            e.meta.status = DemoStatus::Error;
            e.meta.error = Some(message);
        });
        if let Some(demo) = fresh {
            self.notify.notify(Event::DemoChanged { demo });
        }
    }

    /// Path of the replay stream for a parsed demo, building it on first use
    /// (a few seconds: the demo is read again for positions and projectiles).
    pub fn replay_file(self: &Arc<Self>, id: &str) -> Result<PathBuf> {
        let (send, receive) = std::sync::mpsc::channel();
        {
            let mut queue = self.replay_queue.lock().unwrap();
            if let Some(active) = queue.active.as_mut().filter(|job| job.id == id) {
                active.replies.push(send);
            } else if let Some(pending) = queue.pending.iter_mut().find(|job| job.id == id) {
                pending.replies.push(send);
            } else {
                queue.pending.push_back(ReplayRequest {
                    id: id.to_owned(),
                    replies: vec![send],
                });
            }
            if !queue.running {
                queue.running = true;
                let engine = self.clone();
                if let Err(error) = std::thread::Builder::new()
                    .name("replay-builder".into())
                    .spawn(move || engine.run_replay_queue(|id| engine.build_replay_file(id)))
                {
                    queue.running = false;
                    for job in queue.pending.drain(..) {
                        for reply in job.replies {
                            let _ = reply.send(Err(format!("cannot start replay worker: {error}")));
                        }
                    }
                }
            }
        }
        receive
            .recv()
            .map_err(|_| anyhow!("replay worker stopped"))?
            .map_err(|error| anyhow!(error))
    }

    fn run_replay_queue(&self, mut build: impl FnMut(&str) -> Result<PathBuf>) {
        loop {
            let id = {
                let mut queue = self.replay_queue.lock().unwrap();
                let Some(job) = queue.pending.pop_front() else {
                    queue.running = false;
                    return;
                };
                let id = job.id.clone();
                queue.active = Some(job);
                id
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&id)))
                .map_err(|_| "replay builder crashed".to_owned())
                .and_then(|result| result.map_err(|error| format!("{error:#}")));
            let job = self.replay_queue.lock().unwrap().active.take().unwrap();
            for reply in job.replies {
                let _ = reply.send(result.clone());
            }
        }
    }

    fn build_replay_file(&self, id: &str) -> Result<PathBuf> {
        let (meta, parsed) = {
            let _guard = self.data_lock.lock().unwrap();
            let (meta, parsed) = self.get_demo(id).ok_or_else(|| anyhow!("demo not found"))?;
            let parsed = parsed
                .filter(|_| meta.status == DemoStatus::Parsed)
                .ok_or_else(|| anyhow!("demo not parsed"))?;
            let path = self.store.replay_path(id);
            if path.is_file() && self.store.replay_is_current(id) {
                return Ok(path);
            }
            (meta, parsed)
        };
        let bytes = std::fs::read(&meta.path).map_err(|e| anyhow!("reading {}: {e}", meta.path))?;
        let replay = build_replay(&self.parser, &parsed.info, &parsed.rounds, &bytes)?;
        let staged = self.store.stage_replay(&replay)?;
        self.publish_replay(id, &parsed, staged)
    }

    fn publish_replay(
        &self,
        id: &str,
        parsed: &Arc<ParsedDemo>,
        staged: tempfile::NamedTempFile,
    ) -> Result<PathBuf> {
        let _guard = self.data_lock.lock().unwrap();
        anyhow::ensure!(
            self.get_demo(id)
                .is_some_and(|(current, result)| current.status == DemoStatus::Parsed
                    && result.is_some_and(|result| Arc::ptr_eq(&result, parsed))),
            "demo changed while building replay; open it again"
        );
        let path = self.store.replay_path(id);
        staged.persist(&path)?;
        Ok(path)
    }

    /// Radar image(s) + world mapping for a map, extracted from the game files
    /// on first use (or again after a game update).
    pub fn map_assets(&self, map: &str) -> Result<MapAssets> {
        let _tools = self.tool_access.use_tools(&[SetupTool::Vrf]);
        if map.is_empty() || !map.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(anyhow!("unsupported map name {map:?}"));
        }
        let _guard = self.radar_lock.lock().unwrap();
        let tools = self.tool_paths();
        let radar_dir = self.store.radar_dir();
        if let Some(a) = crate::radar::read_map_assets(&radar_dir, map, tools.cs2_patch_version) {
            return Ok(a);
        }
        let cs2 = tools.cs2_dir.ok_or_else(|| anyhow!("CS2 folder not set"))?;
        let vrf = tools.vrf_exe.ok_or_else(|| {
            anyhow!("Source 2 Viewer CLI not installed — use \"Download tools\" in settings")
        })?;
        ensure_map_assets(&radar_dir, &cs2, &vrf, map, tools.cs2_patch_version)
    }

    pub fn active_job_id(&self) -> Option<String> {
        self.active_job
            .lock()
            .unwrap()
            .as_ref()
            .map(|(id, _)| id.clone())
    }

    pub fn clear_match_anomaly(&self, id: &str) -> Result<()> {
        let _guard = self.data_lock.lock().unwrap();
        let _scoring = self
            .scoring_lock
            .try_lock()
            .map_err(|_| anyhow!("analysis is running"))?;
        self.ensure_idle(id)?;
        let mut queue = self.analysis_queue.lock().unwrap();
        anyhow::ensure!(
            !queue.jobs.iter().any(|j| j.demo_id == id
                && matches!(
                    j.status,
                    crate::scoring::queue::Status::Queued | crate::scoring::queue::Status::Running
                )),
            "analysis is queued or running"
        );
        crate::scoring::history::delete_match(&self.data_dir, id)?;
        queue.jobs.retain(|j| j.demo_id != id);
        Ok(())
    }

    fn delete_demo_data(&self, id: &str) -> Result<()> {
        let mut queue = self.analysis_queue.lock().unwrap();
        anyhow::ensure!(
            !queue.jobs.iter().any(|j| j.demo_id == id
                && matches!(
                    j.status,
                    crate::scoring::queue::Status::Queued | crate::scoring::queue::Status::Running
                )),
            "analysis is queued or running"
        );
        auto_attempts::migrate(&self.data_dir, &self.store.list_jobs())?;
        let jobs: HashSet<_> = self
            .store
            .list_jobs()
            .into_iter()
            .filter(|job| job.demo_id == id)
            .map(|job| job.id)
            .collect();
        anyhow::ensure!(!jobs.iter().any(|job|self.job_reservations.lock().unwrap().contains_key(job)),"A review still uses this match's preview files. Finish or cancel that review before removing the match.");
        for job in &jobs {
            self.store.delete_job(job)?;
        }
        self.render_queue
            .lock()
            .unwrap()
            .retain(|job| !jobs.contains(job));
        crate::scoring::history::delete_match(&self.data_dir, id)?;
        self.store.delete_parsed(id)?;
        queue.jobs.retain(|j| j.demo_id != id);
        Ok(())
    }

    /// Clear all demo-owned derived data; preserve the source file.
    pub fn clear_analysis(&self, id: &str) -> Result<()> {
        let _replay_guard = self.data_lock.lock().unwrap();
        let _scoring = self
            .scoring_lock
            .try_lock()
            .map_err(|_| anyhow!("analysis is running"))?;
        self.ensure_idle(id)?;
        self.delete_demo_data(id)?;
        let meta = self
            .update_meta(id, DemoEntry::reset)
            .ok_or_else(|| anyhow!("demo not found"))?;
        self.notify.notify(Event::DemoChanged { demo: meta });
        Ok(())
    }

    /// Delete every parse result (disk + memory). Refused while something is parsing or rendering.
    pub fn clear_all_analysis(&self) -> Result<u64> {
        let _replay_guard = self.data_lock.lock().unwrap();
        if !self.parsing.lock().unwrap().is_empty() {
            return Err(anyhow!("a demo is being parsed"));
        }
        if self.active_job_id().is_some() {
            return Err(anyhow!("a render is running"));
        }
        let _scoring = self
            .scoring_lock
            .try_lock()
            .map_err(|_| anyhow!("analysis is running"))?;
        anyhow::ensure!(
            !self
                .analysis_queue
                .lock()
                .unwrap()
                .jobs
                .iter()
                .any(|j| matches!(
                    j.status,
                    crate::scoring::queue::Status::Queued | crate::scoring::queue::Status::Running
                )),
            "analysis is queued or running"
        );
        let before =
            self.store.parsed_bytes() + self.store.anomaly_bytes() + self.store.clips_bytes();
        let ids: Vec<_> = self.demos.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.delete_demo_data(&id)?;
        }
        self.store.clear_all_parsed();
        self.store.clear_anomaly_data()?;
        for job in self.store.list_jobs() {
            self.store.delete_job(&job.id)?;
        }
        self.render_queue.lock().unwrap().clear();
        self.analysis_queue.lock().unwrap().jobs.clear();
        let freed = before.saturating_sub(
            self.store.parsed_bytes() + self.store.anomaly_bytes() + self.store.clips_bytes(),
        );
        let metas: Vec<DemoMeta> = {
            let mut demos = self.demos.lock().unwrap();
            for e in demos.values_mut() {
                if e.meta.status != DemoStatus::Error {
                    e.reset();
                }
            }
            demos.values().map(|e| e.meta.clone()).collect()
        };
        for meta in metas {
            self.notify.notify(Event::DemoChanged { demo: meta });
        }
        Ok(freed)
    }

    /// Delete the demo, its analysis and all of its render jobs; shared radar maps remain.
    pub fn remove_demo(&self, id: &str) -> Result<()> {
        let _guard = self.data_lock.lock().unwrap();
        let _scoring = self
            .scoring_lock
            .try_lock()
            .map_err(|_| anyhow!("analysis is running"))?;
        self.ensure_idle(id)?;
        let (meta, _) = self.get_demo(id).ok_or_else(|| anyhow!("demo not found"))?;
        self.delete_demo_data(id)?;
        self.store.clear_parse_error(id)?;
        match std::fs::remove_file(&meta.path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(anyhow!("cannot delete {}: {e}", meta.path)),
        }
        {
            let mut registered = self.registered_demos.lock().unwrap();
            let updated: Vec<_> = registered
                .iter()
                .filter(|p| Self::demo_id(p) != id)
                .cloned()
                .collect();
            self.store.save_registered_demos(&updated)?;
            *registered = updated;
        }
        self.demos.lock().unwrap().remove(id);
        Ok(())
    }
    fn ensure_idle(&self, demo_id: &str) -> Result<()> {
        if self.is_parsing(demo_id) {
            return Err(anyhow!("demo is being parsed"));
        }
        if let Some(active) = self.active_job_id() {
            if self
                .store
                .get_job(&active)
                .map(|j| j.demo_id == demo_id)
                .unwrap_or(false)
            {
                return Err(anyhow!("a render for this demo is running"));
            }
        }
        Ok(())
    }

    // ---- setup ----
    pub fn setup_state(&self) -> HashMap<SetupTool, SetupState> {
        self.setup.lock().unwrap().clone()
    }
    pub fn tool_checks(&self) -> HashMap<SetupTool, crate::render::diagnostics::ToolCheck> {
        let paths = self.tool_paths();
        self.tool_checks
            .lock()
            .unwrap()
            .iter()
            .filter(|(tool, check)| {
                check.fingerprint == crate::render::diagnostics::fingerprint(&paths, **tool)
            })
            .map(|(tool, check)| (*tool, check.clone()))
            .collect()
    }
    pub fn check_tools(&self) {
        let _guard = self.tool_check_lock.lock().unwrap();
        for tool in [SetupTool::Hlae, SetupTool::Ffmpeg, SetupTool::Vrf] {
            if self
                .setup
                .lock()
                .unwrap()
                .get(&tool)
                .is_some_and(|s| s.running)
            {
                continue;
            }
            let _tools = self.tool_access.use_tools(&[tool]);
            let check = crate::render::diagnostics::check(&self.tool_paths(), tool);
            self.tool_checks.lock().unwrap().insert(tool, check);
        }
    }
    pub fn check_tool_update(&self, tool: SetupTool) -> Result<crate::render::setup::ToolUpdate> {
        let _tools = self.tool_access.use_tools(&[tool]);
        let exe = crate::render::diagnostics::executable(&self.tool_paths(), tool)
            .cloned()
            .ok_or_else(|| anyhow!("Tool executable not found"))?;
        crate::render::setup::check_update(tool, &exe)
    }
    pub fn tool_diagnostics(&self) -> serde_json::Value {
        use crate::scoring::queue::Status;
        use serde_json::json;

        let settings = self.settings();
        let doctor = doctor(&self.tools_dir(), &self.overrides(&settings));
        let checks = self.tool_checks.lock().unwrap().clone();
        let downloads = self.setup_state();
        let tools: serde_json::Map<String, serde_json::Value> = [
            ("hlae", SetupTool::Hlae),
            ("ffmpeg", SetupTool::Ffmpeg),
            ("vrf", SetupTool::Vrf),
        ]
        .into_iter()
        .map(|(name, tool)| {
            let mut report =
                crate::render::diagnostics::tool_report(&doctor.paths, tool, checks.get(&tool));
            if let Some(download) = downloads.get(&tool) {
                report["download"] = json!({ "running": download.running,
                    "stopping": download.stopping, "progress": download.progress,
                    "lastMessage": download.log.last() });
            }
            (name.into(), report)
        })
        .collect();
        let parsing = {
            let demos = self.demos.lock().unwrap();
            json!({
                "known": demos.len(),
                "new": demos.values().filter(|d| d.meta.status == DemoStatus::New).count(),
                "queued": demos.values().filter(|d| d.meta.status == DemoStatus::Queued).count(),
                "validating": demos.values().filter(|d| d.meta.status == DemoStatus::Validating).count(),
                "parsing": demos.values().filter(|d| d.meta.status == DemoStatus::Parsing).count(),
                "parsed": demos.values().filter(|d| d.meta.status == DemoStatus::Parsed).count(),
                "failed": demos.values().filter(|d| d.meta.status == DemoStatus::Error).count(),
                "errorSamples": demos.values().filter(|d| d.meta.status == DemoStatus::Error)
                    .take(3).map(|d| json!({ "demoId": d.meta.id, "path": d.meta.path, "error": d.meta.error }))
                    .collect::<Vec<_>>()
            })
        };
        let analysis = self.analysis_jobs();
        let analysis_detail = |j: &crate::scoring::queue::Job| {
            json!({
                "jobId": j.id, "demoId": j.demo_id, "step": j.step,
                "startedAt": j.started_at, "finishedAt": j.finished_at, "error": j.error
            })
        };
        let render = self.store.list_jobs();
        let render_detail = |j: &RenderJob| {
            json!({
                "jobId": j.id, "demoId": j.demo_id, "stage": j.stage, "progress": j.progress,
                "startedAt": j.started_at, "finishedAt": j.finished_at,
                "errorCode": j.error_code, "error": j.error,
                "logTail": &j.log[j.log.len().saturating_sub(5)..]
            })
        };
        json!({
            "generatedAt": now(), "environment": crate::render::diagnostics::environment(),
            "dataDirectory": self.data_dir(), "paths": doctor.paths,
            "configuredPaths": self.overrides(&settings), "problems": doctor.problems,
            "toolCacheScope": "session", "tools": tools,
            "activity": {
                "parsing": parsing,
                "analysis": {
                    "scope": "session",
                    "queued": analysis.iter().filter(|j| j.status == Status::Queued).count(),
                    "failed": analysis.iter().filter(|j| j.status == Status::Error).count(),
                    "active": analysis.iter().find(|j| j.status == Status::Running).map(analysis_detail),
                    "latestFailure": analysis.iter().filter(|j| j.status == Status::Error)
                        .max_by_key(|j| j.finished_at.as_ref().unwrap_or(&j.created_at)).map(analysis_detail)
                },
                "render": {
                    "queued": self.render_queue.lock().unwrap().len(),
                    "failed": render.iter().filter(|j| j.status == JobStatus::Error).count(),
                    "active": render.iter().find(|j| j.status == JobStatus::Running).map(render_detail),
                    "latestFailure": render.iter().filter(|j| j.status == JobStatus::Error)
                        .max_by_key(|j| j.finished_at.as_ref().unwrap_or(&j.created_at)).map(render_detail)
                }
            }
        })
    }
    pub fn start_setup(
        self: &Arc<Self>,
        tool: SetupTool,
        force: bool,
        directory: Option<PathBuf>,
    ) -> bool {
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut downloads = self.setup.lock().unwrap();
            let state = downloads.entry(tool).or_default();
            if state.running {
                return false;
            }
            *state = SetupState {
                running: true,
                stopping: false,
                progress: None,
                log: vec!["Download started".into()],
                cancel: cancel.clone(),
            };
        }
        let engine = self.clone();
        std::thread::spawn(move || {
            let settings = engine.store.settings();
            let previous_selection = match tool {
                SetupTool::Hlae => settings.hlae_exe.clone(),
                SetupTool::Ffmpeg => settings.ffmpeg_exe.clone(),
                SetupTool::Vrf => settings.vrf_exe.clone(),
            };
            let mut overrides = engine.overrides(&settings);
            match tool {
                SetupTool::Hlae => overrides.hlae_exe = directory.clone(),
                SetupTool::Ffmpeg => overrides.ffmpeg_exe = directory.clone(),
                SetupTool::Vrf => overrides.vrf_exe = directory.clone(),
            }
            let mut log = |line: String| {
                engine
                    .setup
                    .lock()
                    .unwrap()
                    .get_mut(&tool)
                    .unwrap()
                    .progress = Some(line.clone());
                engine.notify.notify(Event::SetupProgress {
                    tool,
                    progress: line,
                });
            };
            // Keep staging on the target volume so publishing is a directory rename.
            let workspace = directory
                .as_deref()
                .map(crate::render::paths::installation_directory)
                .unwrap_or_else(|| engine.tools_dir())
                .join(format!(".install-{tool:?}"));
            let journal = engine.tools_dir().join(format!(".install-{tool:?}.json"));
            let mut replacement = None;
            let mut before_replace = || {
                replacement = Some(engine.tool_access.replace(tool, &cancel)?);
                Ok(())
            };
            let mut installed = false;
            let result = (|| {
                std::fs::create_dir_all(engine.tools_dir())?;
                std::fs::write(&journal, serde_json::to_vec(&workspace)?)?;
                run_setup(
                    &engine.tools_dir(),
                    &overrides,
                    tool,
                    force,
                    &mut crate::render::setup::Progress {
                        cancel: &cancel,
                        report: &mut log,
                        workspace: Some(&workspace),
                        before_replace: Some(&mut before_replace),
                    },
                )
            })()
            .and_then(|_| {
                let _guard = engine.settings_lock.lock().unwrap();
                let mut settings = engine.store.settings();
                let selected = directory
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned());
                let current = match tool {
                    SetupTool::Hlae => &mut settings.hlae_exe,
                    SetupTool::Ffmpeg => &mut settings.ffmpeg_exe,
                    SetupTool::Vrf => &mut settings.vrf_exe,
                };
                if *current == previous_selection {
                    *current = selected;
                    engine.store.save_settings(&settings)?;
                }
                installed = true;
                drop(_guard);
                let check = crate::render::diagnostics::check(&engine.tool_paths(), tool);
                let error = check.error.clone();
                engine.tool_checks.lock().unwrap().insert(tool, check);
                if let Some(error) = error {
                    anyhow::bail!("Tool installed but startup verification failed: {error}");
                }
                Ok(())
            });
            let _ = std::fs::remove_file(journal);
            drop(replacement);
            let cancelled = result.is_err() && cancel.load(Ordering::Relaxed);
            {
                let mut downloads = engine.setup.lock().unwrap();
                let state = downloads.get_mut(&tool).unwrap();
                state.running = false;
                state.stopping = false;
                state.progress = None;
                state.log.push(match &result {
                    Ok(_) => "Download completed".into(),
                    Err(_) if cancelled => "Download cancelled".into(),
                    Err(e) => format!("error: {e:#}"),
                });
            }
            match result {
                Ok(_) => engine.notify.notify(Event::SetupFinished {
                    tool,
                    ok: true,
                    installed,
                    cancelled: false,
                    error: None,
                }),
                Err(e) => engine.notify.notify(Event::SetupFinished {
                    tool,
                    ok: false,
                    installed,
                    cancelled,
                    error: Some(format!("{e:#}")),
                }),
            }
        });
        true
    }

    pub fn cancel_setup(&self, tool: SetupTool) {
        if let Some(state) = self.setup.lock().unwrap().get_mut(&tool) {
            if state.running {
                state.stopping = true;
                state.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    // ---- render queue ----
    pub fn enqueue_render(
        self: &Arc<Self>,
        demo_id: &str,
        highlight_ids: Vec<String>,
        options: RenderOptions,
    ) -> Result<RenderJob> {
        let _guard = self.data_lock.lock().unwrap();
        self.enqueue_render_locked(demo_id, highlight_ids, options, None)
    }

    fn enqueue_render_locked(
        self: &Arc<Self>,
        demo_id: &str,
        highlight_ids: Vec<String>,
        options: RenderOptions,
        automatic_attempts: Option<&mut auto_attempts::Ledger>,
    ) -> Result<RenderJob> {
        options.validate()?;
        let (meta, parsed) = self
            .get_demo(demo_id)
            .ok_or_else(|| anyhow!("demo not found"))?;
        if meta.status != DemoStatus::Parsed || parsed.is_none() {
            return Err(anyhow!("demo not parsed"));
        }
        if highlight_ids.is_empty() {
            return Err(anyhow!("pick at least one highlight"));
        }
        check_ascii_path(Path::new(&meta.path))?;
        let parsed = parsed.unwrap();
        let highlights: Vec<_> = parsed
            .highlights
            .iter()
            .filter(|h| highlight_ids.contains(&h.id))
            .cloned()
            .collect();
        anyhow::ensure!(
            highlights.len() == highlight_ids.len(),
            "The selected moments changed. Open this match and select them again."
        );
        let snapshot = source_snapshot(&meta, &parsed.info, highlights, &parsed.demo_digest)?;
        if let Some(ledger) = automatic_attempts {
            ledger.reserve(&self.data_dir, demo_id, &snapshot)?;
        }
        let mut job = self.store.new_job(demo_id, highlight_ids, options)?;
        job.total = Some(snapshot.highlights.len());
        job.source_snapshot = Some(Box::new(snapshot));
        self.store.save_job(&job)?;
        self.render_queue.lock().unwrap().push_back(job.id.clone());
        self.notify.notify(Event::JobChanged {
            job: Box::new(job.clone()),
        });
        self.pump();
        Ok(job)
    }

    /// Automatic scans are idempotent: an already exported/queued moment is not re-recorded.
    pub fn enqueue_automatic_render(
        self: &Arc<Self>,
        demo_id: &str,
        mut highlight_ids: Vec<String>,
        options: RenderOptions,
    ) -> Result<Option<RenderJob>> {
        let _guard = self.data_lock.lock().unwrap();
        let (meta, parsed) = self
            .get_demo(demo_id)
            .ok_or_else(|| anyhow!("demo not found"))?;
        let parsed = parsed.ok_or_else(|| anyhow!("demo not parsed"))?;
        let fingerprint = format!("sha256:{}", parsed.demo_digest);
        let mut ledger = auto_attempts::migrate(&self.data_dir, &self.store.list_jobs())?;
        ledger.resolve_legacy_source(&self.data_dir, demo_id, Path::new(&meta.path), &parsed)?;
        highlight_ids.retain(|id| {
            parsed
                .highlights
                .iter()
                .find(|highlight| highlight.id == *id)
                .is_none_or(|highlight| {
                    !ledger.contains_highlight(demo_id, &fingerprint, highlight)
                })
        });
        highlight_ids.sort();
        highlight_ids.dedup();
        if highlight_ids.is_empty() {
            return Ok(None);
        }
        self.enqueue_render_locked(demo_id, highlight_ids, options, Some(&mut ledger))
            .map(Some)
    }

    /// Explicit custom windows use the same validated recording path as detected highlights.
    /// Internal review preparation is separate from an exported gameplay moment.
    pub fn enqueue_review_preview(
        self: &Arc<Self>,
        demo_id: &str,
        player_id: &str,
        windows: Vec<[i32; 2]>,
        title: String,
        options: RenderOptions,
    ) -> Result<(RenderJob, JobReservation)> {
        let _guard = self.data_lock.lock().unwrap();
        let job = self.enqueue_custom_render_locked(
            demo_id,
            player_id,
            windows,
            title,
            options,
            "ai-preview",
            None,
            None,
        )?;
        let reservation = JobReservation {
            engine: Arc::downgrade(self),
            id: job.id.clone(),
        };
        Ok((job, reservation))
    }

    pub fn enqueue_custom_render(
        self: &Arc<Self>,
        demo_id: &str,
        player_id: &str,
        windows: Vec<[i32; 2]>,
        title: String,
        options: RenderOptions,
    ) -> Result<RenderJob> {
        let _guard = self.data_lock.lock().unwrap();
        self.enqueue_custom_render_locked(
            demo_id, player_id, windows, title, options, "custom", None, None,
        )
    }

    pub fn enqueue_custom_render_with_origin(
        self: &Arc<Self>,
        demo_id: &str,
        player_id: &str,
        windows: Vec<[i32; 2]>,
        title: String,
        options: RenderOptions,
        origin: &str,
    ) -> Result<RenderJob> {
        anyhow::ensure!(
            ["custom", "blooper"].contains(&origin),
            "Unsupported moment origin."
        );
        let _guard = self.data_lock.lock().unwrap();
        self.enqueue_custom_render_locked(demo_id, player_id, windows, title, options, origin, None, None)
    }

    /// Candidate exports use registered source/player/window facts, not caller
    /// supplied labels. Speculative whiffs keep their candidate qualification.
    pub fn enqueue_discovery_render(
        self: &Arc<Self>, demo_id: &str, player_id: &str, candidate_ids: Vec<String>,
        windows: Vec<[i32; 2]>, title: String, options: RenderOptions,
    ) -> Result<RenderJob> {
        let _guard = self.data_lock.lock().unwrap();
        let parsed = self.parsed(demo_id).ok_or_else(|| anyhow!("Parse this match before exporting discoveries."))?;
        let candidates = validated_discovery_selection(&parsed, player_id, &candidate_ids, &windows)?;
        self.enqueue_custom_render_locked(demo_id, player_id, windows, title, options, "discovery", None, Some(&candidates))
    }

    pub fn enqueue_automatic_custom_render(
        self: &Arc<Self>,
        demo_id: &str,
        player_id: &str,
        mut windows: Vec<[i32; 2]>,
        title: String,
        options: RenderOptions,
    ) -> Result<Option<RenderJob>> {
        let _guard = self.data_lock.lock().unwrap();
        let (meta, parsed) = self
            .get_demo(demo_id)
            .ok_or_else(|| anyhow!("demo not found"))?;
        let parsed = parsed.ok_or_else(|| anyhow!("demo not parsed"))?;
        let fingerprint = format!("sha256:{}", parsed.demo_digest);
        let mut ledger = auto_attempts::migrate(&self.data_dir, &self.store.list_jobs())?;
        ledger.resolve_legacy_source(&self.data_dir, demo_id, Path::new(&meta.path), &parsed)?;
        windows.retain(|window| {
            !ledger.contains_window(demo_id, &fingerprint, player_id, window, &parsed.highlights)
        });
        if windows.is_empty() {
            return Ok(None);
        }
        self.enqueue_custom_render_locked(
            demo_id,
            player_id,
            windows,
            title,
            options,
            "blooper",
            Some(&mut ledger),
            None,
        )
        .map(Some)
    }

    fn enqueue_custom_render_locked(
        self: &Arc<Self>,
        demo_id: &str,
        player_id: &str,
        windows: Vec<[i32; 2]>,
        title: String,
        mut options: RenderOptions,
        origin: &str,
        automatic_attempts: Option<&mut auto_attempts::Ledger>,
        discovery_candidates: Option<&[crate::model::DiscoveryCandidate]>,
    ) -> Result<RenderJob> {
        options.validate()?;
        let (meta, parsed) = self
            .get_demo(demo_id)
            .ok_or_else(|| anyhow!("demo not found"))?;
        let parsed = parsed.ok_or_else(|| anyhow!("demo not parsed"))?;
        let player = parsed
            .info
            .players
            .iter()
            .find(|p| p.steamid == player_id)
            .ok_or_else(|| anyhow!("player not found in this demo"))?;
        let end = parsed
            .rounds
            .iter()
            .map(|r| r.end_tick.max(r.officially_ended_tick))
            .max()
            .ok_or_else(|| anyhow!("demo timeline unavailable"))?;
        anyhow::ensure!(
            !windows.is_empty() && windows.len() <= 512,
            "select valid clip windows"
        );
        anyhow::ensure!(
            !title.is_empty() && title.chars().count() <= 160,
            "invalid clip title"
        );
        let mut snapshot = source_snapshot(&meta, &parsed.info, Vec::new(), &parsed.demo_digest)?;
        let fingerprint = snapshot.fingerprint.clone();
        let mut highlights = Vec::new();
        for (index, [start, finish]) in windows.into_iter().enumerate() {
            anyhow::ensure!(
                start >= 0 && finish > start && finish <= end,
                "clip is outside the demo timeline"
            );
            let round = parsed
                .rounds
                .iter()
                .find(|r| r.start_tick <= start && r.end_tick >= start)
                .map(|r| r.round)
                .unwrap_or(0);
            let candidate = discovery_candidates.and_then(|items| items.get(index));
            highlights.push(crate::Highlight {
                id: candidate.map(|c| c.id.clone()).unwrap_or_else(|| format!(
                    "custom-{}",
                    sha1_smol::Sha1::from(format!(
                        "{fingerprint}:{player_id}:{start}:{finish}:{title}"
                    ))
                    .digest()
                )),
                player: crate::model::HighlightPlayer {
                    steamid: player_id.into(),
                    name: player.name.clone(),
                },
                round,
                start_tick: start,
                end_tick: finish,
                anchor_tick: candidate.map_or(start, |c| c.anchor_tick),
                key_moments: Vec::new(),
                round_result: None,
                score: 0.0,
                tags: if let Some(candidate) = candidate {
                    let mut tags = candidate.tags.clone();
                    for tag in ["discovery", candidate.category.as_str()] {
                        if !tags.iter().any(|t| t == tag) { tags.push(tag.to_string()); }
                    }
                    tags
                } else if origin == "ai-preview" {
                    vec!["custom".into(), "ai-preview".into()]
                } else {
                    vec![origin.into()]
                },
                title: candidate.map_or_else(|| title.clone(), |c| c.title.clone()),
                kills: Vec::new(),
                breakdown: Default::default(),
            });
        }
        check_ascii_path(Path::new(&meta.path))?;
        options.key_moments_only = false;
        let group = crate::scoring::clips::RuleClips {
            rule_id: if matches!(origin, "ai-preview" | "discovery") {
                origin.into()
            } else {
                "custom".into()
            },
            title,
            demo_fingerprint: fingerprint,
            highlights,
        };
        snapshot.highlights = group.highlights.clone();
        snapshot.discovery_candidates = discovery_candidates.unwrap_or(&[]).to_vec();
        if let Some(ledger) = automatic_attempts {
            ledger.reserve(&self.data_dir, demo_id, &snapshot)?;
        }
        let mut job = self.store.new_job(
            demo_id,
            group.highlights.iter().map(|h| h.id.clone()).collect(),
            options,
        )?;
        job.total = Some(snapshot.highlights.len());
        job.source_snapshot = Some(Box::new(snapshot));
        job.analysis_clips = Some(Box::new(group));
        self.store.save_job(&job)?;
        if origin == "ai-preview" {
            self.job_reservations
                .lock()
                .unwrap()
                .insert(job.id.clone(), 1);
        }
        self.render_queue.lock().unwrap().push_back(job.id.clone());
        self.persist(&job);
        self.pump();
        Ok(job)
    }

    pub fn analysis_clips(
        &self,
        demo_id: &str,
        selection: &crate::scoring::clips::Selection,
        merge: bool,
    ) -> Result<Vec<crate::scoring::clips::RuleClips>> {
        let (_, parsed) = self
            .get_demo(demo_id)
            .ok_or_else(|| anyhow!("demo not found"))?;
        let parsed = parsed.ok_or_else(|| anyhow!("demo not parsed"))?;
        let record = self
            .scoring_history(demo_id, &selection.player_id)?
            .into_iter()
            .find(|r| r.id == selection.assessment_id)
            .ok_or_else(|| anyhow!("analysis record not found"))?;
        let end = parsed
            .rounds
            .iter()
            .map(|r| r.end_tick.max(r.officially_ended_tick))
            .max()
            .ok_or_else(|| anyhow!("demo timeline unavailable"))?;
        let groups = crate::scoring::clips::build(&record, &parsed.info, end, &selection.rule_ids)?;
        Ok(if merge {
            crate::scoring::clips::merge(groups, parsed.info.tick_rate)
        } else {
            groups
        })
    }

    pub fn enqueue_analysis_render(
        self: &Arc<Self>,
        demo_id: &str,
        selection: crate::scoring::clips::Selection,
        mut options: RenderOptions,
    ) -> Result<Vec<RenderJob>> {
        let _guard = self.data_lock.lock().unwrap();
        options.validate()?;
        let groups = self.analysis_clips(demo_id, &selection, options.merge)?;
        let (meta, parsed) = self
            .get_demo(demo_id)
            .ok_or_else(|| anyhow!("demo not found"))?;
        check_ascii_path(Path::new(&meta.path))?;
        let parsed = parsed.ok_or_else(|| anyhow!("demo not parsed"))?;
        let base_snapshot = source_snapshot(&meta, &parsed.info, Vec::new(), &parsed.demo_digest)?;
        let mut verified_analysis = HashSet::new();
        options.merge = true;
        let mut jobs: Vec<RenderJob> = Vec::new();
        let result = (|| -> Result<()> {
            for group in groups {
                if verified_analysis.insert(group.demo_fingerprint.clone()) {
                    verify_fingerprint(Path::new(&meta.path), &group.demo_fingerprint)?;
                }
                let mut snapshot = base_snapshot.clone();
                snapshot.highlights = group.highlights.clone();
                let mut job = self.store.new_job(
                    demo_id,
                    group.highlights.iter().map(|h| h.id.clone()).collect(),
                    options.clone(),
                )?;
                job.total = Some(snapshot.highlights.len());
                job.source_snapshot = Some(Box::new(snapshot));
                job.analysis_clips = Some(Box::new(group));
                jobs.push(job);
                self.store.save_job(jobs.last().unwrap())?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            for mut job in jobs {
                job.status = JobStatus::Error;
                job.error = Some(format!("Export batch was not queued: {error:#}"));
                job.finished_at = Some(now());
                self.persist(&job);
            }
            return Err(error);
        }
        self.render_queue
            .lock()
            .unwrap()
            .extend(jobs.iter().map(|j| j.id.clone()));
        for job in &jobs {
            self.notify.notify(Event::JobChanged {
                job: Box::new(job.clone()),
            });
        }
        self.pump();
        Ok(jobs)
    }

    /// A retry is a fresh job; old output files and their original options stay intact.
    pub fn open_render_source(self: &Arc<Self>, id: &str) -> Result<DemoMeta> {
        anyhow::ensure!(
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "Invalid export job."
        );
        let job = self
            .store
            .get_job(id)
            .ok_or_else(|| anyhow!("This export job is missing."))?;
        if let Some(snapshot) = &job.source_snapshot {
            let path = Path::new(&snapshot.demo_path);
            anyhow::ensure!(path.is_absolute() && Self::is_dem(path) && path.canonicalize().is_ok_and(|p| p.is_file()), "The original source demo is missing. Import its unchanged .dem file to open this match.");
            verify_fingerprint(path, &snapshot.fingerprint)?;
            // Preserve the stored path spelling: canonical Windows extended paths have a different legacy demo ID.
            self.add_demo(path)
        } else {
            let meta=self.list_demos().into_iter().find(|demo| demo.id == job.demo_id).ok_or_else(|| anyhow!("This older export has no saved source path. Import its original demo and open it from Matches."))?;
            anyhow::ensure!(Path::new(&meta.path).is_file(),"The original source demo is missing. Import its .dem file and open it from Matches.");
            if let Some(clips) = &job.analysis_clips {
                verify_fingerprint(Path::new(&meta.path), &clips.demo_fingerprint)?;
            }
            Ok(meta)
        }
    }

    pub fn retry_render_job(self: &Arc<Self>, id: &str) -> Result<RenderJob> {
        let _guard = self.data_lock.lock().unwrap();
        anyhow::ensure!(
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "Invalid export job."
        );
        let old = self
            .store
            .get_job(id)
            .ok_or_else(|| anyhow!("This export job is missing."))?;
        anyhow::ensure!(
            !matches!(old.status, JobStatus::Queued | JobStatus::Running),
            "This export is already queued or running."
        );
        old.options.validate()?;
        let snapshot = match &old.source_snapshot {
            Some(value) => (**value).clone(),
            None => {
                let clips = old.analysis_clips.as_ref().ok_or_else(|| anyhow!("This older export has no saved recording windows. Open its match and select the moments to export again."))?;
                let (meta, parsed) = self.get_demo(&old.demo_id).ok_or_else(|| {
                    anyhow!("The source match is missing. Import it again before retrying.")
                })?;
                let parsed = parsed.ok_or_else(|| anyhow!("Open and parse this source match before retrying its older analysis export."))?;
                verify_fingerprint(Path::new(&meta.path), &clips.demo_fingerprint)?;
                SourceSnapshot {
                    demo_path: meta.path,
                    fingerprint: clips.demo_fingerprint.clone(),
                    demo_info: parsed.info.clone(),
                    highlights: clips.highlights.clone(),
                    discovery_candidates: Vec::new(),
                }
            }
        };
        verify_fingerprint(Path::new(&snapshot.demo_path), &snapshot.fingerprint)?;
        check_ascii_path(Path::new(&snapshot.demo_path))?;
        let mut job =
            self.store
                .new_job(&old.demo_id, old.highlight_ids.clone(), old.options.clone())?;
        job.total = Some(snapshot.highlights.len());
        job.source_snapshot = Some(Box::new(snapshot));
        job.analysis_clips = old.analysis_clips.clone();
        job.retry_of = Some(old.id);
        self.store.save_job(&job)?;
        self.render_queue.lock().unwrap().push_back(job.id.clone());
        self.persist(&job);
        self.pump();
        Ok(job)
    }

    pub fn get_clip_media(&self, job_id: &str, output_key: &str) -> Result<ClipMedia> {
        let _tools = self.tool_access.use_tools(&[SetupTool::Ffmpeg]);
        self.media_worker.get(
            &self.store,
            &self.data_dir,
            job_id,
            output_key,
            self.tool_paths().ffmpeg_exe.as_deref(),
            &self.shutting_down,
        )
    }

    pub fn cancel_clip_media(&self, job_id: &str, output_key: &str) {
        self.media_worker.cancel(job_id, output_key);
    }

    pub fn cancel_job(&self, id: &str) -> bool {
        {
            let mut q = self.render_queue.lock().unwrap();
            if let Some(pos) = q.iter().position(|j| j == id) {
                q.remove(pos);
                if let Some(mut job) = self.store.get_job(id) {
                    job.status = JobStatus::Cancelled;
                    job.finished_at = Some(now());
                    self.persist(&job);
                }
                return true;
            }
        }
        if let Some((active, flag)) = self.active_job.lock().unwrap().as_ref() {
            if active == id {
                flag.store(true, Ordering::SeqCst);
                if let Some(mut job) = self.store.get_job(id) {
                    job.stage = Some("cancelling".into());
                    self.persist(&job);
                }
                return true;
            }
        }
        false
    }

    pub fn reserve_job(self: &Arc<Self>, id: &str) -> Result<JobReservation> {
        let _guard = self.data_lock.lock().unwrap();
        anyhow::ensure!(
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'),
            "Invalid export job."
        );
        anyhow::ensure!(
            self.store.get_job(id).is_some(),
            "The review preview job is missing."
        );
        *self
            .job_reservations
            .lock()
            .unwrap()
            .entry(id.into())
            .or_default() += 1;
        Ok(JobReservation {
            engine: Arc::downgrade(self),
            id: id.into(),
        })
    }

    pub fn delete_job(&self, id: &str) -> Result<()> {
        let _guard = self.data_lock.lock().unwrap();
        anyhow::ensure!(!self.job_reservations.lock().unwrap().contains_key(id),"A review still uses this preview. Finish or cancel that review before deleting its files.");
        if self.active_job_id().as_deref() == Some(id) {
            return Err(anyhow!("job is running"));
        }
        auto_attempts::migrate(&self.data_dir, &self.store.list_jobs())?;
        self.cancel_job(id);
        self.media_worker.cancel_job(id);
        self.media_worker.while_idle(|| self.store.delete_job(id))
    }

    /// Delete every render job and its videos. Refused while a render is running or queued.
    pub fn clear_all_clips(&self) -> Result<u64> {
        let _guard = self.data_lock.lock().unwrap();
        anyhow::ensure!(self.job_reservations.lock().unwrap().is_empty(),"A review still uses preview files. Finish or cancel that review before clearing clips.");
        if self.active_job_id().is_some() || !self.render_queue.lock().unwrap().is_empty() {
            return Err(anyhow!("a render is running"));
        }
        auto_attempts::migrate(&self.data_dir, &self.store.list_jobs())?;
        self.media_worker.cancel_all();
        Ok(self
            .media_worker
            .while_idle(|| self.store.clear_all_clips()))
    }

    fn pump(self: &Arc<Self>) {
        if self.render_worker_running.swap(true, Ordering::SeqCst) {
            return;
        }
        let engine = self.clone();
        let spawned = std::thread::Builder::new()
            .name("render-worker".into())
            .spawn(move || {
                loop {
                    if engine.shutting_down.load(Ordering::SeqCst) {
                        engine.render_worker_running.store(false, Ordering::SeqCst);
                        return;
                    }
                    let paused = engine.queue_paused();
                    let game_busy = crate::render::cs2_is_running().unwrap_or(true);
                    if paused || game_busy {
                        let queued_id = engine.render_queue.lock().unwrap().front().cloned();
                        if let Some(id) = queued_id {
                            if let Some(mut job) = engine.store.get_job(&id) {
                                let stage = if paused { "paused" } else { "waiting-for-game" };
                                if job.stage.as_deref() != Some(stage) {
                                    job.stage = Some(stage.into());
                                    engine.persist(&job);
                                }
                            }
                        } else {
                            // Serialize worker release with enqueue, including an empty paused queue.
                            let queue = engine.render_queue.lock().unwrap();
                            if queue.is_empty() {
                                engine.render_worker_running.store(false, Ordering::SeqCst);
                                return;
                            }
                        }
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        continue;
                    }
                    let id = {
                        let mut queue = engine.render_queue.lock().unwrap();
                        let Some(id) = queue.pop_front() else {
                            // Enqueue cannot observe an empty queue with a worker still marked busy.
                            engine.render_worker_running.store(false, Ordering::SeqCst);
                            return;
                        };
                        id
                    };
                    if let Some(job) = engine.store.get_job(&id) {
                        if job.status == JobStatus::Queued {
                            engine.run_job(job);
                        }
                    }
                }
            });
        if let Err(e) = spawned {
            eprintln!("could not start the render worker: {e}");
            let ids: Vec<_> = self.render_queue.lock().unwrap().drain(..).collect();
            for id in ids {
                if let Some(mut job) = self.store.get_job(&id) {
                    job.status = JobStatus::Error;
                    job.error = Some(format!("Cannot start render worker: {e}"));
                    job.finished_at = Some(now());
                    self.persist(&job);
                }
            }
            self.render_worker_running.store(false, Ordering::SeqCst);
        }
    }

    /// Write a job record and tell the UI; a failed write is logged, not fatal.
    fn persist(&self, job: &RenderJob) {
        if let Err(e) = self.store.save_job(job) {
            eprintln!("could not save job {}: {e:#}", job.id);
        }
        self.notify.notify(Event::JobChanged {
            job: Box::new(job.clone()),
        });
    }

    fn run_job(self: &Arc<Self>, mut job: RenderJob) {
        let _tools = self
            .tool_access
            .use_tools(&[SetupTool::Hlae, SetupTool::Ffmpeg]);
        // Serialize claiming a queued job with demo removal, including already-dequeued jobs.
        let guard = self.data_lock.lock().unwrap();
        if !self
            .store
            .get_job(&job.id)
            .is_some_and(|j| j.status == JobStatus::Queued)
        {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *self.active_job.lock().unwrap() = Some((job.id.clone(), cancel.clone()));
        drop(guard);
        job.status = JobStatus::Running;
        job.started_at = Some(now());
        job.stage = Some("starting".into());
        job.progress = Some(0.0);
        self.persist(&job);

        let progress = Mutex::new(job.clone());
        let mut log = |line: String| {
            let mut j = progress.lock().unwrap();
            if cancel.load(Ordering::SeqCst) {
                j.stage = Some("cancelling".into());
            }
            j.log.push(line);
            if j.log.len() > 400 {
                let excess = j.log.len() - 400;
                j.log.drain(0..excess);
            }
            self.persist(&j);
        };
        let mut stage = |s: &str| {
            let mut j = progress.lock().unwrap();
            j.stage = Some(
                if cancel.load(Ordering::SeqCst) {
                    "cancelling"
                } else {
                    s
                }
                .to_string(),
            );
            self.persist(&j);
        };
        let mut report_progress = |value: f64| {
            let mut j = progress.lock().unwrap();
            if cancel.load(Ordering::SeqCst) {
                j.stage = Some("cancelling".into());
            }
            let value = value.clamp(0.0, 0.99).max(j.progress.unwrap_or(0.0));
            if value - j.progress.unwrap_or(0.0) < 0.005 {
                return;
            }
            j.progress = Some(value);
            self.persist(&j);
        };
        let outcome = if let Some(snapshot) = &job.source_snapshot {
            verify_fingerprint(Path::new(&snapshot.demo_path), &snapshot.fingerprint).and_then(
                |()| {
                    render_highlights(RenderJobInput {
                        demo: &snapshot.demo_info,
                        demo_path: PathBuf::from(&snapshot.demo_path),
                        highlights: snapshot.highlights.clone(),
                        preserve_merge_order: job.analysis_clips.is_some(),
                        output_dir: self.store.job_dir(&job.id),
                        options: job.options.clone(),
                        tools: self.tool_paths(),
                        cancel: cancel.clone(),
                        log: &mut log,
                        stage: &mut stage,
                        progress: &mut report_progress,
                    })
                },
            )
        } else {
            self.get_demo(&job.demo_id)
                .ok_or_else(|| anyhow!("demo not found"))
                .and_then(|(meta, parsed)| {
                    let parsed = parsed.ok_or_else(|| {
                        anyhow!("demo not parsed (parse it again after restarting the app)")
                    })?;
                    let wanted: HashSet<&str> =
                        job.highlight_ids.iter().map(|s| s.as_str()).collect();
                    let highlights = if let Some(clips) = &job.analysis_clips {
                        use std::io::Read;
                        let mut source = std::fs::File::open(&meta.path)?;
                        let mut hash = sha1_smol::Sha1::new();
                        let mut buffer = [0u8; 65536];
                        loop {
                            let n = source.read(&mut buffer)?;
                            if n == 0 {
                                break;
                            }
                            hash.update(&buffer[..n]);
                        }
                        anyhow::ensure!(
                            format!("sha1:{}", hash.digest()) == clips.demo_fingerprint,
                            "demo content changed since analysis; analyze again before exporting"
                        );
                        clips.highlights.clone()
                    } else {
                        parsed
                            .highlights
                            .iter()
                            .filter(|h| wanted.contains(h.id.as_str()))
                            .cloned()
                            .collect()
                    };
                    render_highlights(RenderJobInput {
                        demo: &parsed.info,
                        demo_path: PathBuf::from(&meta.path),
                        highlights,
                        preserve_merge_order: job.analysis_clips.is_some(),
                        output_dir: self.store.job_dir(&job.id),
                        options: job.options.clone(),
                        tools: self.tool_paths(),
                        cancel: cancel.clone(),
                        log: &mut log,
                        stage: &mut stage,
                        progress: &mut report_progress,
                    })
                })
        };
        let latest = progress.into_inner().unwrap();
        job.log = latest.log;
        job.progress = latest.progress;

        let _guard = self.data_lock.lock().unwrap();
        job.finished_at = Some(now());
        job.stage = None;
        match outcome {
            Ok(result) => {
                apply_render_result(&mut job, &result);
            }
            Err(e) => {
                let msg = format!("{e:#}");
                job.status = if msg == "cancelled" {
                    JobStatus::Cancelled
                } else {
                    JobStatus::Error
                };
                job.error = Some(msg);
            }
        }
        self.persist(&job);
        *self.active_job.lock().unwrap() = None;
    }
}

fn apply_render_result(job: &mut RenderJob, result: &crate::render::RenderResult) {
    job.outputs = job_outputs(result);
    job.completed = Some(
        result
            .clips
            .iter()
            .filter(|clip| clip.file.is_some() || result.final_video.is_some())
            .count(),
    );
    job.failed_highlights = result.failed_highlights.clone();
    if let Some(clips) = &job.analysis_clips {
        for output in &mut job.outputs {
            if output.is_final {
                output.title = clips.title.clone();
            }
        }
    }
    if !job.outputs.is_empty() {
        job.progress = Some(1.0);
    }
    let failure = result.failure_details.first().cloned();
    job.status = if job.outputs.is_empty() {
        JobStatus::Error
    } else if !job.failed_highlights.is_empty() || failure.is_some() {
        JobStatus::Partial
    } else {
        JobStatus::Done
    };
    job.error = if job.outputs.is_empty() {
        Some(failure.unwrap_or_else(|| {
            "No clip was recorded. Check this export's logs and retry it.".into()
        }))
    } else if job.status == JobStatus::Partial {
        failure
    } else {
        None
    };
}

fn verify_fingerprint(path: &Path, expected: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|_| anyhow!("The source demo is missing or cannot be read. Import the original match before retrying."))?;
    let mut sha256 = Sha256::new();
    let mut sha1 = sha1_smol::Sha1::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        if expected.starts_with("sha256:") {
            sha256.update(&buffer[..n]);
        } else if expected.starts_with("sha1:") {
            sha1.update(&buffer[..n]);
        } else {
            anyhow::bail!(
                "The saved source fingerprint is unsupported. Open the match and export again."
            );
        }
    }
    let actual = if expected.starts_with("sha256:") {
        format!("sha256:{:x}", sha256.finalize())
    } else {
        format!("sha1:{}", sha1.digest())
    };
    anyhow::ensure!(actual == expected, "The source demo changed since this export was selected. Open the match and select its moments again.");
    Ok(())
}

fn source_snapshot(
    meta: &DemoMeta,
    info: &crate::model::DemoInfo,
    highlights: Vec<crate::model::Highlight>,
    expected_digest: &str,
) -> Result<SourceSnapshot> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(&meta.path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    let digest = format!("{:x}", hash.finalize());
    anyhow::ensure!(
        expected_digest.is_empty() || expected_digest == digest,
        "The demo changed after parsing. Open it and parse again before exporting."
    );
    Ok(SourceSnapshot {
        demo_path: meta.path.clone(),
        fingerprint: format!("sha256:{digest}"),
        demo_info: info.clone(),
        highlights,
        discovery_candidates: Vec::new(),
    })
}

fn validated_discovery_selection(
    parsed: &ParsedDemo, player: &str, ids: &[String], windows: &[[i32; 2]],
) -> Result<Vec<crate::model::DiscoveryCandidate>> {
    anyhow::ensure!(!ids.is_empty() && ids.len() <= 512 && ids.len() == windows.len(), "Choose matching discovery candidates and their registered windows.");
    let mut seen = HashSet::new();
    ids.iter().zip(windows).map(|(id, window)| {
        anyhow::ensure!(seen.insert(id), "A discovery candidate was selected more than once.");
        let candidate = parsed.discovery_candidates.iter().find(|c| &c.id == id).ok_or_else(|| anyhow!("The discovery candidate is no longer registered. Reopen the match."))?;
        anyhow::ensure!(candidate.player_id == player && !parsed.demo_digest.is_empty() && candidate.source_digest == parsed.demo_digest,
            "The discovery candidate belongs to a different player or source.");
        anyhow::ensure!(*window == [candidate.start_tick, candidate.end_tick], "The requested discovery window differs from its registered source evidence.");
        Ok(candidate.clone())
    }).collect()
}

/// The game's `playdemo` only takes ASCII paths; refuse early with a clear message.
fn check_ascii_path(path: &Path) -> Result<()> {
    if path.to_string_lossy().is_ascii() {
        Ok(())
    } else {
        Err(anyhow!("CS2 can only play demos from an ASCII path (playdemo limitation) — move or rename it: {}", path.display()))
    }
}

/// One row per clip file, or the single merged video.
fn job_outputs(result: &crate::render::RenderResult) -> Vec<JobOutput> {
    let mut outputs: Vec<JobOutput> = result
        .clips
        .iter()
        .filter_map(|c| {
            c.file.as_ref().map(|f| JobOutput {
                file: f.to_string_lossy().to_string(),
                bytes: c.bytes.unwrap_or(0),
                highlight_id: Some(c.highlight_id.clone()),
                title: c.title.clone(),
                is_final: false,
            })
        })
        .collect();
    if let (Some(f), Some(b)) = (&result.final_video, result.final_bytes) {
        outputs.push(JobOutput {
            file: f.to_string_lossy().to_string(),
            bytes: b,
            highlight_id: None,
            title: "highlights (merged)".into(),
            is_final: true,
        });
    }
    outputs
}

#[cfg(test)]
mod redesign_backend_tests {
    use super::*;
    use serde_json::json;
    struct Silent;
    impl Notify for Silent {
        fn notify(&self, _: Event) {}
    }
    #[test]
    fn discovery_exports_freeze_factual_provenance_and_reject_forged_identity_windows_and_changed_sources() {
        let (_root, engine) = fixture();
        let mut parsed = (*engine.parsed("demo").unwrap()).clone();
        let candidate: crate::model::DiscoveryCandidate = serde_json::from_value(json!({"id":"candidate-whiff","sourceDigest":parsed.demo_digest,
            "playerId":"1","round":1,"startTick":100,"endTick":200,"anchorTick":150,"category":"whiff_candidate","title":"Possible whiff — recorded misses",
            "tags":["whiff_candidate","visual-review-needed"],"origins":["demo-events-v1"],"evidence":[{"eventId":"fire-150","tick":150,"kind":"weapon_fire","weapon":"awp"}],"requiresVisualReview":true})).unwrap();
        parsed.discovery_candidates.push(candidate.clone());
        engine.demos.lock().unwrap().get_mut("demo").unwrap().parsed = Some(Arc::new(parsed));
        let export = engine.enqueue_discovery_render("demo","1",vec![candidate.id.clone()],vec![[100,200]],"Discovery selection".into(),Default::default()).unwrap();
        let saved = engine.store.get_job(&export.id).unwrap();
        let source = saved.source_snapshot.as_ref().unwrap();
        assert_eq!(source.discovery_candidates[0].evidence[0].event_id,"fire-150");
        assert_eq!(source.highlights[0].id,"candidate-whiff");
        assert!(source.highlights[0].title.starts_with("Possible whiff"));
        assert!(source.highlights[0].tags.contains(&"whiff_candidate".into()));
        for (player, ids, windows) in [("other",vec![candidate.id.clone()],vec![[100,200]]),("1",vec!["not-registered".into()],vec![[100,200]]),
            ("1",vec![candidate.id.clone()],vec![[100,201]]),("1",vec![candidate.id.clone(),candidate.id.clone()],vec![[100,200],[100,200]])] {
            assert!(engine.enqueue_discovery_render("demo",player,ids,windows,"Forged selection".into(),Default::default()).is_err());
        }
        let mut old = saved.clone(); old.status=JobStatus::Error;engine.store.save_job(&old).unwrap();
        let retry=engine.retry_render_job(&old.id).unwrap();
        assert_eq!(retry.source_snapshot.as_ref().unwrap().discovery_candidates[0].id,candidate.id);
        std::fs::write(&source.demo_path,b"changed source fixture").unwrap();
        assert!(engine.enqueue_discovery_render("demo","1",vec![candidate.id],vec![[100,200]],"Changed source".into(),Default::default()).is_err());
        assert!(engine.retry_render_job(&old.id).is_err());
    }
    #[test]
    fn completed_preview_reservation_protects_only_its_files_until_review_cleanup() {
        let (_root, engine) = fixture();
        let (mut preview, first) = engine
            .enqueue_review_preview(
                "demo",
                "1",
                vec![[100, 200]],
                "Preview reservation fixture".into(),
                Default::default(),
            )
            .unwrap();
        engine.cancel_job(&preview.id);
        preview.status = JobStatus::Done;
        let file = engine
            .store
            .job_dir(&preview.id)
            .join("owned-preview-fixture.mp4");
        std::fs::write(&file, b"synthetic deletion fixture, not gameplay").unwrap();
        preview.outputs = vec![JobOutput {
            file: file.to_string_lossy().into_owned(),
            bytes: 39,
            highlight_id: Some("fixture".into()),
            title: "Preview fixture".into(),
            is_final: false,
        }];
        engine.store.save_job(&preview).unwrap();
        assert!(engine.active_job_id().is_none());
        assert!(engine
            .delete_job(&preview.id)
            .unwrap_err()
            .to_string()
            .contains("review"));
        assert!(engine.clear_all_clips().is_err());
        assert!(engine.remove_demo("demo").is_err());
        assert!(file.exists());
        let unrelated = engine
            .enqueue_render("demo", vec!["moment".into()], Default::default())
            .unwrap();
        engine.cancel_job(&unrelated.id);
        engine.delete_job(&unrelated.id).unwrap();
        assert!(file.exists());
        let second = engine.reserve_job(&preview.id).unwrap();
        drop(first);
        assert!(engine.delete_job(&preview.id).is_err());
        drop(second);
        engine.delete_job(&preview.id).unwrap();
        assert!(!file.exists());
        assert!(engine.job_reservations.lock().unwrap().is_empty());
    }
    #[test]
    fn internal_review_previews_do_not_suppress_real_automatic_exports_and_retries_keep_purpose() {
        let (_root, engine) = fixture();
        let profile = RenderOptions {
            width: 1280,
            height: 720,
            fps: 15,
            merge: false,
            key_moments_only: false,
            ..Default::default()
        };
        let (mut preview, _reservation) = engine
            .enqueue_review_preview(
                "demo",
                "1",
                vec![[100, 200], [300, 400]],
                "Preview fixture".into(),
                profile,
            )
            .unwrap();
        assert_eq!(
            preview.analysis_clips.as_ref().unwrap().rule_id,
            "ai-preview"
        );
        assert!(preview
            .source_snapshot
            .as_ref()
            .unwrap()
            .highlights
            .iter()
            .all(|highlight| highlight.tags.iter().any(|tag| tag == "ai-preview")));
        let actual = engine
            .enqueue_automatic_render("demo", vec!["moment".into()], Default::default())
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                actual.options.width,
                actual.options.height,
                actual.options.fps
            ),
            (3840, 2160, 60)
        );
        assert!(engine
            .enqueue_automatic_custom_render(
                "demo",
                "1",
                vec![[300, 400]],
                "Actual suggestion".into(),
                Default::default()
            )
            .unwrap()
            .is_some());
        preview.status = JobStatus::Error;
        engine.store.save_job(&preview).unwrap();
        let retried = engine.retry_render_job(&preview.id).unwrap();
        assert_eq!(
            (
                retried.options.width,
                retried.options.height,
                retried.options.fps
            ),
            (1280, 720, 15)
        );
        assert_eq!(
            retried.analysis_clips.as_ref().unwrap().rule_id,
            "ai-preview"
        );
        assert!(retried
            .source_snapshot
            .as_ref()
            .unwrap()
            .highlights
            .iter()
            .all(|highlight| highlight.tags.iter().any(|tag| tag == "ai-preview")));
    }
    #[test]
    fn automatic_ledger_survives_deleted_records_changed_defaults_restart_and_legacy_jobs() {
        let (root, engine) = fixture();
        let first = engine
            .enqueue_automatic_render("demo", vec!["moment".into()], Default::default())
            .unwrap()
            .unwrap();
        engine.delete_job(&first.id).unwrap();
        let quality = RenderOptions {
            width: 1920,
            height: 1080,
            codec: "libx264".into(),
            ..Default::default()
        };
        let mut settings = engine.settings();
        settings.render_defaults = quality.clone();
        engine.save_settings(settings).unwrap();
        assert!(engine
            .enqueue_automatic_render("demo", vec!["moment".into()], quality.clone())
            .unwrap()
            .is_none());
        let custom = engine
            .enqueue_automatic_custom_render(
                "demo",
                "1",
                vec![[300, 400]],
                "First description".into(),
                Default::default(),
            )
            .unwrap()
            .unwrap();
        engine.cancel_job(&custom.id);
        let mut legacy = engine
            .store
            .new_job("demo", vec!["legacy-marker".into()], Default::default())
            .unwrap();
        legacy.status = JobStatus::Done;
        engine.store.save_job(&legacy).unwrap();
        let mut legacy_analysis = engine
            .store
            .new_job("demo", vec!["legacy-ai".into()], Default::default())
            .unwrap();
        let mut highlight = engine.parsed("demo").unwrap().highlights[0].clone();
        highlight.id = "legacy-ai".into();
        highlight.start_tick = 500;
        highlight.end_tick = 600;
        legacy_analysis.status = JobStatus::Error;
        legacy_analysis.analysis_clips = Some(Box::new(crate::scoring::clips::RuleClips {
            rule_id: "custom".into(),
            title: "Legacy window".into(),
            demo_fingerprint: format!(
                "sha1:{}",
                sha1_smol::Sha1::from(b"source recording fixture").digest()
            ),
            highlights: vec![highlight],
        }));
        engine.store.save_job(&legacy_analysis).unwrap();
        assert!(engine
            .enqueue_automatic_custom_render(
                "demo",
                "1",
                vec![[500, 600]],
                "New model description".into(),
                quality.clone()
            )
            .unwrap()
            .is_none());
        engine.clear_all_clips().unwrap();
        assert!(engine.store.list_jobs().is_empty());
        let history: serde_json::Value = serde_json::from_slice(
            &std::fs::read(engine.data_dir.join("automatic-attempts.json")).unwrap(),
        )
        .unwrap();
        let legacy = history["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["momentId"] == "legacy-marker")
            .unwrap();
        assert_eq!(legacy["kind"], "legacy");
        assert!(legacy.get("playerId").is_none());
        assert!(legacy.get("sourceFingerprint").is_none());
        let source = engine.demos.lock().unwrap()["demo"].clone();
        let restarted = Engine::new(root.path().join("data"), Arc::new(Silent)).unwrap();
        restarted
            .render_worker_running
            .store(true, Ordering::SeqCst);
        restarted
            .demos
            .lock()
            .unwrap()
            .insert("demo".into(), source);
        assert_eq!(restarted.recording_defaults().width, 1920);
        assert!(restarted
            .enqueue_automatic_render("demo", vec!["moment".into()], quality.clone())
            .unwrap()
            .is_none());
        assert!(restarted
            .enqueue_automatic_custom_render(
                "demo",
                "1",
                vec![[300, 400]],
                "Changed title after restart".into(),
                quality.clone()
            )
            .unwrap()
            .is_none());
        assert!(restarted
            .enqueue_automatic_custom_render(
                "demo",
                "1",
                vec![[500, 600]],
                "Changed legacy title".into(),
                quality
            )
            .unwrap()
            .is_none());
        assert!(restarted.store.list_jobs().is_empty());
    }
    #[test]
    fn corrupt_automatic_history_stops_auto_without_blocking_manual_export_or_startup() {
        let (root, engine) = fixture();
        std::fs::write(
            engine.data_dir.join("automatic-attempts.json"),
            b"{broken history",
        )
        .unwrap();
        assert!(engine
            .enqueue_automatic_render("demo", vec!["moment".into()], Default::default())
            .unwrap_err()
            .to_string()
            .contains("corrupt"));
        assert!(engine.store.list_jobs().is_empty());
        let manual = engine
            .enqueue_render("demo", vec!["moment".into()], Default::default())
            .unwrap();
        assert_eq!(manual.status, JobStatus::Queued);
        assert!(engine.get_demo("demo").is_some());
        let restarted = Engine::new(root.path().join("data"), Arc::new(Silent)).unwrap();
        assert!(restarted.store.get_job(&manual.id).is_some());
        assert!(
            std::fs::read(engine.data_dir.join("automatic-attempts.json"))
                .unwrap()
                .starts_with(b"{broken")
        );
    }
    #[cfg(windows)]
    #[test]
    fn failed_atomic_attempt_save_retains_previous_history_and_never_queues_the_new_export() {
        use std::os::windows::fs::OpenOptionsExt;
        let (_root, engine) = fixture();
        let first = engine
            .enqueue_automatic_render("demo", vec!["moment".into()], Default::default())
            .unwrap()
            .unwrap();
        engine.delete_job(&first.id).unwrap();
        let path = engine.data_dir.join("automatic-attempts.json");
        let original = std::fs::read(&path).unwrap();
        // An owned read handle prevents replacing this one fixture file on Windows.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        let error = engine
            .enqueue_automatic_custom_render(
                "demo",
                "1",
                vec![[1000, 1100]],
                "Atomic failure fixture".into(),
                Default::default(),
            )
            .unwrap_err();
        assert!(format!("{error:#}").contains("not queued"));
        assert!(engine.render_queue.lock().unwrap().is_empty());
        assert!(engine.store.list_jobs().is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(!std::fs::read_dir(&engine.data_dir)
            .unwrap()
            .flatten()
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with("automatic-attempts-")
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "tmp")));
        drop(held);
        assert!(engine
            .enqueue_automatic_render("demo", vec!["moment".into()], Default::default())
            .unwrap()
            .is_none());
        assert!(engine
            .enqueue_automatic_custom_render(
                "demo",
                "1",
                vec![[1000, 1100]],
                "Atomic retry fixture".into(),
                Default::default()
            )
            .unwrap()
            .is_some());
    }
    #[cfg(windows)]
    #[test]
    fn heavy_analysis_waits_for_shared_compute_and_shutdown_interrupts_waiting() {
        let (_root, engine) = fixture();
        engine.analysis_queue.lock().unwrap().enqueue("demo", false);
        engine.analysis_queue.lock().unwrap().claim().unwrap();
        let held = crate::render::lease::ComputeLease::try_acquire()
            .unwrap()
            .expect("run this isolated test after native recording is closed");
        let worker_engine = engine.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = worker_engine.scoring_compute_lease("demo");
            send.send(result.is_ok()).unwrap();
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while engine.analysis_jobs()[0].phase
            != Some(crate::scoring::queue::Phase::WaitingForCompute)
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            engine.analysis_jobs()[0].phase,
            Some(crate::scoring::queue::Phase::WaitingForCompute)
        );
        assert!(receive.try_recv().is_err());
        drop(held);
        assert!(receive
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap());
        worker.join().unwrap();
        assert_eq!(
            engine.analysis_jobs()[0].phase,
            Some(crate::scoring::queue::Phase::Preparing)
        );
        let held = crate::render::lease::ComputeLease::try_acquire()
            .unwrap()
            .unwrap();
        let worker_engine = engine.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = worker_engine.scoring_compute_lease("demo");
            send.send(result.is_err()).unwrap();
        });
        engine.shutting_down.store(true, Ordering::SeqCst);
        assert!(receive
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap());
        worker.join().unwrap();
        drop(held);
    }
    #[test]
    fn failed_recording_cause_is_visible_and_successful_partial_outputs_stay_registered() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path().into()).unwrap();
        let mut job = store
            .new_job(
                "fixture",
                vec!["good".into(), "failed".into()],
                Default::default(),
            )
            .unwrap();
        let cause = "Windows could not allocate enough virtual memory for the recording process (error 1455). Close other memory-heavy applications, then retry this export.";
        let mut result = crate::render::RenderResult {
            final_video: None,
            final_bytes: None,
            clips: vec![
                crate::render::RenderedClip {
                    highlight_id: "good".into(),
                    title: "Completed fixture".into(),
                    file: Some(root.path().join("completed.mp4")),
                    bytes: Some(42),
                },
                crate::render::RenderedClip {
                    highlight_id: "failed".into(),
                    title: "Failed fixture".into(),
                    file: None,
                    bytes: None,
                },
            ],
            failed_highlights: vec!["Failed fixture".into()],
            failure_details: vec![cause.into()],
        };
        apply_render_result(&mut job, &result);
        assert_eq!(job.status, JobStatus::Partial);
        assert_eq!(job.completed, Some(1));
        assert_eq!(job.outputs.len(), 1);
        assert_eq!(job.outputs[0].highlight_id.as_deref(), Some("good"));
        assert_eq!(job.error.as_deref(), Some(cause));
        store.save_job(&job).unwrap();
        let saved = store.get_job(&job.id).unwrap();
        assert_eq!(saved.outputs.len(), 1);
        assert_eq!(saved.error.as_deref(), Some(cause));
        result.clips[0].file = None;
        apply_render_result(&mut job, &result);
        assert_eq!(job.status, JobStatus::Error);
        assert!(job.outputs.is_empty());
        assert_eq!(job.error.as_deref(), Some(cause));
        result.failure_details.clear();
        apply_render_result(&mut job, &result);
        assert!(job
            .error
            .as_deref()
            .unwrap()
            .contains("Check this export's logs"));
    }
    fn fixture() -> (tempfile::TempDir, Arc<Engine>) {
        use sha2::{Digest, Sha256};
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.dem");
        std::fs::write(&source, b"source recording fixture").unwrap();
        let info = json!({"path":source,"mapName":"de_fixture","serverName":"fixture","tickRate":64,"players":[{"steamid":"1","name":"Fixture player","teamNumber":3,"userId":0}]});
        let parsed = serde_json::from_value(json!({"demoDigest":format!("{:x}", Sha256::digest(b"source recording fixture")),"info":info,"rounds":[{"round":1,"startTick":0,"freezeEndTick":0,"endTick":6400,"officiallyEndedTick":6400,"reason":"","roster":{}}],"kills":[],"highlights":[{"id":"moment","player":{"steamid":"1","name":"Fixture player"},"round":1,"startTick":100,"endTick":200,"anchorTick":150,"score":1,"tags":["death"],"title":"Selected death","kills":[],"breakdown":{}}],"stats":[],"score":{},"roundSummaries":[],"parsedAt":""})).unwrap();
        let meta = serde_json::from_value(json!({"id":"demo","name":"source.dem","path":source,"bytes":24,"mtimeMs":0,"createdMs":0,"status":"parsed"})).unwrap();
        let data = root.path().join("data");
        Store::open(data.clone())
            .unwrap()
            .save_settings(&Settings {
                scan_game_replays: false,
                ..Default::default()
            })
            .unwrap();
        let engine = Engine::new(data, Arc::new(Silent)).unwrap();
        engine.render_worker_running.store(true, Ordering::SeqCst);
        engine.demos.lock().unwrap().insert(
            "demo".into(),
            DemoEntry {
                meta,
                parsed: Some(Arc::new(parsed)),
                auto_complete: None,
            },
        );
        (root, engine)
    }
    #[test]
    fn legacy_settings_migrate_exact_defaults_and_invalid_defaults_cannot_save() {
        let (root, engine) = fixture();
        let legacy: Settings = serde_json::from_str("{\"scanGameReplays\":false}").unwrap();
        assert_eq!(
            serde_json::to_value(legacy.render_defaults).unwrap(),
            serde_json::to_value(RenderOptions::default()).unwrap()
        );
        let mut settings = engine.settings();
        settings.render_defaults.width = 1920;
        settings.render_defaults.height = 1080;
        settings.render_defaults.voice = false;
        engine.save_settings(settings.clone()).unwrap();
        let reopened = Engine::new(root.path().join("data"), Arc::new(Silent)).unwrap();
        assert_eq!(reopened.recording_defaults().width, 1920);
        assert!(!reopened.recording_defaults().voice);
        settings
            .render_defaults
            .extra_launch_options
            .push("+connect invalid".into());
        assert!(engine.save_settings(settings).is_err());
        assert_eq!(engine.recording_defaults().width, 1920);
    }
    #[test]
    fn retries_preserve_original_windows_options_outputs_and_reject_changed_source() {
        let (root, engine) = fixture();
        let mut original = engine
            .enqueue_render("demo", vec!["moment".into()], RenderOptions::default())
            .unwrap();
        assert_eq!(
            original.source_snapshot.as_ref().unwrap().highlights[0].tags,
            vec!["death"]
        );
        let video = engine.store.job_dir(&original.id).join("kept.mp4");
        std::fs::write(&video, b"original output").unwrap();
        original.status = JobStatus::Partial;
        original.outputs.push(JobOutput {
            file: video.to_string_lossy().into_owned(),
            bytes: 15,
            highlight_id: Some("moment".into()),
            title: "Saved output".into(),
            is_final: false,
        });
        engine.store.save_job(&original).unwrap();
        let mut defaults = engine.settings();
        defaults.render_defaults.width = 1920;
        defaults.render_defaults.height = 1080;
        engine.save_settings(defaults).unwrap();
        let retry = engine.retry_render_job(&original.id).unwrap();
        assert_ne!(retry.id, original.id);
        assert_eq!(retry.retry_of.as_deref(), Some(original.id.as_str()));
        assert_eq!(retry.options.width, 3840);
        assert_eq!(
            retry.source_snapshot.as_ref().unwrap().highlights[0].start_tick,
            100
        );
        assert!(retry.outputs.is_empty());
        assert_eq!(std::fs::read(&video).unwrap(), b"original output");
        original.status = JobStatus::Error;
        engine.store.save_job(&original).unwrap();
        std::fs::write(root.path().join("source.dem"), b"changed recording bytes!").unwrap();
        assert!(engine
            .retry_render_job(&original.id)
            .unwrap_err()
            .to_string()
            .contains("changed"));
        assert!(engine
            .enqueue_render("demo", vec!["moment".into()], Default::default())
            .unwrap_err()
            .to_string()
            .contains("changed"));
        assert!(engine.retry_render_job("../escape").is_err());
    }
    #[test]
    fn automatic_attempts_block_reexports_after_quality_changes_failures_and_cancel() {
        let (_root, engine) = fixture();
        let mut job = engine
            .enqueue_automatic_render("demo", vec!["moment".into()], Default::default())
            .unwrap()
            .unwrap();
        let options = RenderOptions {
            width: 1920,
            height: 1080,
            ..Default::default()
        };
        for status in [
            JobStatus::Queued,
            JobStatus::Running,
            JobStatus::Error,
            JobStatus::Cancelled,
            JobStatus::Partial,
            JobStatus::Done,
        ] {
            job.status = status;
            engine.store.save_job(&job).unwrap();
            assert!(engine
                .enqueue_automatic_render("demo", vec!["moment".into()], options.clone())
                .unwrap()
                .is_none());
        }
        let mut custom = engine
            .enqueue_automatic_custom_render(
                "demo",
                "1",
                vec![[300, 400]],
                "First model description".into(),
                Default::default(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            custom.source_snapshot.as_ref().unwrap().highlights[0].tags,
            vec!["blooper"]
        );
        custom.status = JobStatus::Cancelled;
        engine.store.save_job(&custom).unwrap();
        assert!(engine
            .enqueue_automatic_custom_render(
                "demo",
                "1",
                vec![[300, 400]],
                "Different model description".into(),
                options
            )
            .unwrap()
            .is_none());
        let mut renamed = engine.demos.lock().unwrap()["demo"].clone();
        renamed.meta.id = "renamed-demo".into();
        engine
            .demos
            .lock()
            .unwrap()
            .insert("renamed-demo".into(), renamed);
        assert!(engine
            .enqueue_automatic_render("renamed-demo", vec!["moment".into()], Default::default())
            .unwrap()
            .is_none());
        assert!(engine
            .enqueue_automatic_custom_render(
                "renamed-demo",
                "1",
                vec![[300, 400]],
                "Same moment after rename".into(),
                Default::default()
            )
            .unwrap()
            .is_none());
    }
    #[test]
    fn older_jobs_load_without_snapshot_and_legacy_analysis_retry_is_supported() {
        let (_root, engine) = fixture();
        let mut old = engine
            .store
            .new_job("demo", vec!["moment".into()], Default::default())
            .unwrap();
        old.status = JobStatus::Error;
        engine.store.save_job(&old).unwrap();
        assert!(engine
            .retry_render_job(&old.id)
            .unwrap_err()
            .to_string()
            .contains("older export"));
        let highlight = engine.parsed("demo").unwrap().highlights[0].clone();
        old.analysis_clips = Some(Box::new(crate::scoring::clips::RuleClips {
            rule_id: "fixture".into(),
            title: "Old analysis".into(),
            demo_fingerprint: format!(
                "sha1:{}",
                sha1_smol::Sha1::from(b"source recording fixture").digest()
            ),
            highlights: vec![highlight],
        }));
        engine.store.save_job(&old).unwrap();
        let retry = engine.retry_render_job(&old.id).unwrap();
        assert!(retry.source_snapshot.is_some());
        assert!(retry.analysis_clips.is_some());
        let mut json = serde_json::to_value(&old).unwrap();
        for field in ["sourceSnapshot", "retryOf", "completed", "total"] {
            json.as_object_mut().unwrap().remove(field);
        }
        json["schemaVersion"] = json!(2);
        let loaded: RenderJob = serde_json::from_value(json).unwrap();
        assert!(loaded.source_snapshot.is_none());
        assert_eq!(loaded.schema_version, 2);
    }

    #[test]
    fn opening_export_source_recovers_registration_after_restart_and_rejects_missing_or_changed_files(
    ) {
        let (root, engine) = fixture();
        let mut job = engine
            .enqueue_render("demo", vec!["moment".into()], Default::default())
            .unwrap();
        job.status = JobStatus::Done;
        engine.store.save_job(&job).unwrap();
        assert!(engine.store.registered_demos().unwrap().is_empty());
        let restarted = Engine::new(root.path().join("data"), Arc::new(Silent)).unwrap();
        assert!(restarted.list_demos().is_empty());
        let recovered = restarted.open_render_source(&job.id).unwrap();
        assert_eq!(Path::new(&recovered.path), root.path().join("source.dem"));
        assert!(restarted
            .store
            .registered_demos()
            .unwrap()
            .contains(&root.path().join("source.dem")));
        let again = Engine::new(root.path().join("data"), Arc::new(Silent)).unwrap();
        assert!(again
            .list_demos()
            .iter()
            .any(|demo| demo.id == recovered.id));
        std::fs::write(root.path().join("source.dem"), b"changed bytes").unwrap();
        assert!(restarted
            .open_render_source(&job.id)
            .unwrap_err()
            .to_string()
            .contains("changed"));
        std::fs::remove_file(root.path().join("source.dem")).unwrap();
        assert!(restarted
            .open_render_source(&job.id)
            .unwrap_err()
            .to_string()
            .contains("missing"));
        assert!(restarted.open_render_source("../invalid").is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    struct Events(mpsc::Sender<Event>);
    impl Notify for Events {
        fn notify(&self, event: Event) {
            let _ = self.0.send(event);
        }
    }

    #[test]
    fn startup_rechecks_interrupted_custom_download_and_keeps_old_tools_on_hash_failure() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let custom = root.path().join("custom");
        let dir = custom.join("hlae");
        let work = custom.join(".install-Hlae");
        std::fs::create_dir_all(dir.join("x64")).unwrap();
        std::fs::write(dir.join("HLAE.exe"), b"old exe").unwrap();
        std::fs::write(dir.join("x64/AfxHookSource2.dll"), b"old dll").unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("download.zip"), b"incomplete zip").unwrap();
        std::fs::write(work.join("pending.json"), serde_json::to_vec(&serde_json::json!({
            "directory": dir, "url": "https://github.com/advancedfx/advancedfx/releases/download/test/hlae.zip",
            "tag": "test", "digest": format!("sha256:{}", "0".repeat(64)),
        })).unwrap()).unwrap();
        std::fs::create_dir_all(data.join("tools")).unwrap();
        std::fs::write(
            data.join("tools/.install-Hlae.json"),
            serde_json::to_vec(&work).unwrap(),
        )
        .unwrap();
        let (send, receive) = mpsc::channel();
        let engine = Engine::new(data, Arc::new(Events(send))).unwrap();
        loop {
            if let Event::SetupFinished {
                ok,
                installed,
                error,
                ..
            } = receive.recv_timeout(Duration::from_secs(5)).unwrap()
            {
                assert!(!ok && !installed);
                assert!(error.unwrap().contains("SHA-256"));
                break;
            }
        }
        assert!(!engine.setup_state()[&SetupTool::Hlae].running);
        assert_eq!(std::fs::read(dir.join("HLAE.exe")).unwrap(), b"old exe");
        assert_eq!(
            std::fs::read(dir.join("x64/AfxHookSource2.dll")).unwrap(),
            b"old dll"
        );
        assert!(!work.exists());
    }

    #[test]
    fn diagnostics_summarizes_failures_without_starting_work() {
        use crate::scoring::queue::Status;
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let (send, receive) = mpsc::channel();
        let engine = Engine::new(temp.path().join("data"), Arc::new(Events(send))).unwrap();
        for index in 0..5 {
            let id = format!("demo-{index}");
            let meta = serde_json::from_value(json!({
                "id": id, "name": "fixture.dem", "path": "fixture.dem",
                "bytes": 0, "mtimeMs": 0, "createdMs": 0,
                "status": if index == 0 { "new" } else { "error" },
                "error": if index == 0 { None } else { Some("fixture parse failure") }
            }))
            .unwrap();
            engine.demos.lock().unwrap().insert(
                id,
                DemoEntry {
                    meta,
                    parsed: None,
                    auto_complete: None,
                },
            );
        }
        {
            let mut queue = engine.analysis_queue.lock().unwrap();
            queue.enqueue("demo-1", false);
            queue.jobs[0].status = Status::Error;
            queue.jobs[0].error = Some("fixture analysis failure".into());
            queue.enqueue("demo-2", false);
            queue.claim().unwrap();
            queue.enqueue("demo-3", false);
        }
        let mut job = engine
            .store
            .new_job("demo-1", vec![], RenderOptions::default())
            .unwrap();
        job.status = JobStatus::Error;
        job.error = Some("fixture encoding failure".into());
        job.created_at = "2026-09-17T00:00:00Z".into();
        job.finished_at = Some("2026-09-17T00:03:00Z".into());
        job.log = (0..10).map(|n| format!("stage {n}")).collect();
        engine.store.save_job(&job).unwrap();
        let mut earlier_failure = engine
            .store
            .new_job("demo-2", vec![], RenderOptions::default())
            .unwrap();
        earlier_failure.status = JobStatus::Error;
        earlier_failure.error = Some("earlier failure from newer job".into());
        earlier_failure.created_at = "2026-09-17T00:01:00Z".into();
        earlier_failure.finished_at = Some("2026-09-17T00:02:00Z".into());
        engine.store.save_job(&earlier_failure).unwrap();
        let record = engine.store.job_dir(&job.id).join("job.json");
        let before = std::fs::read(&record).unwrap();

        let report = engine.tool_diagnostics();
        let activity = &report["activity"];
        assert_eq!(activity["parsing"]["known"], 5);
        assert_eq!(activity["parsing"]["new"], 1);
        assert_eq!(activity["parsing"]["failed"], 4);
        assert_eq!(
            activity["parsing"]["errorSamples"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(activity["analysis"]["queued"], 1);
        assert_eq!(activity["analysis"]["active"]["demoId"], "demo-2");
        assert_eq!(
            activity["analysis"]["latestFailure"]["error"],
            "fixture analysis failure"
        );
        let failure = &activity["render"]["latestFailure"];
        assert_eq!(activity["render"]["failed"], 2);
        assert_eq!(failure["error"], "fixture encoding failure");
        assert_eq!(
            failure["logTail"],
            json!(["stage 5", "stage 6", "stage 7", "stage 8", "stage 9"])
        );
        assert!(failure.get("options").is_none() && failure.get("outputs").is_none());
        for tool in ["hlae", "ffmpeg", "vrf"] {
            assert_eq!(report["tools"][tool]["cache"], "missing");
            assert!(report["tools"][tool]["lastCheck"].is_null());
        }
        assert!(engine.tool_checks.lock().unwrap().is_empty());
        assert!(engine.parsing.lock().unwrap().is_empty());
        assert!(receive.try_recv().is_err());
        assert_eq!(std::fs::read(record).unwrap(), before);
    }

    #[test]
    fn setup_switches_only_the_installed_tool_to_managed_path() {
        let temp = tempfile::tempdir().unwrap();
        let (send, receive) = mpsc::channel();
        let engine = Engine::new(temp.path().join("data"), Arc::new(Events(send))).unwrap();
        let empty = temp.path().join("empty-tool-parent");
        std::fs::create_dir(&empty).unwrap();
        let selection = Some(empty.to_string_lossy().into_owned());
        engine
            .save_settings(Settings {
                hlae_exe: selection.clone(),
                ffmpeg_exe: selection.clone(),
                vrf_exe: selection.clone(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(engine.settings().hlae_exe, selection);
        let paths = engine.tool_paths();
        assert!(paths.hlae_exe.is_none() && paths.ffmpeg_exe.is_none() && paths.vrf_exe.is_none());
        engine.save_settings(Settings::default()).unwrap();
        for file in [
            "hlae/HLAE.exe",
            "hlae/x64/AfxHookSource2.dll",
            "ffmpeg/ffmpeg.exe",
            "ffmpeg/ffprobe.exe",
        ] {
            let path = engine.tools_dir().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, []).unwrap();
        }
        let vrf = engine
            .tools_dir()
            .join("vrf")
            .join(crate::render::setup::vrf_exe_name());
        std::fs::create_dir_all(vrf.parent().unwrap()).unwrap();
        std::fs::write(&vrf, []).unwrap();
        // Keep both workers active at the settings write boundary without network access.
        let settings_guard = engine.settings_lock.lock().unwrap();
        assert!(engine.start_setup(SetupTool::Hlae, false, None));
        assert!(engine.start_setup(SetupTool::Ffmpeg, false, None));
        assert!(!engine.start_setup(SetupTool::Hlae, false, None));
        let active = engine.setup_state();
        assert!(active[&SetupTool::Hlae].running && active[&SetupTool::Ffmpeg].running);
        drop(settings_guard);
        for _ in 0..2 {
            assert!(matches!(
                receive.recv_timeout(Duration::from_secs(5)).unwrap(),
                Event::SetupFinished { ok: false, .. }
            ));
        }
        for tool in [SetupTool::Hlae, SetupTool::Ffmpeg, SetupTool::Vrf] {
            let mut expected = Settings {
                language: Some("ja".into()),
                hlae_exe: Some("missing-hlae.exe".into()),
                ffmpeg_exe: Some("missing-ffmpeg.exe".into()),
                vrf_exe: Some("missing-vrf.exe".into()),
                ..Settings::default()
            };
            engine.store.save_settings(&expected).unwrap();
            expected = engine.settings();
            assert!(engine.start_setup(tool, false, None));
            loop {
                if let Event::SetupFinished { ok, error, .. } =
                    receive.recv_timeout(Duration::from_secs(5)).unwrap()
                {
                    assert!(!ok && error.unwrap().contains("startup verification failed"));
                    break;
                }
            }
            let paths = engine.tool_paths();
            let setup = engine.setup_state().remove(&tool).unwrap();
            assert!(!setup.running);
            assert!(setup.progress.is_none());
            assert_eq!(setup.log[0], "Download started");
            assert!(setup.log[1].contains("startup verification failed"));
            assert!(!engine.tool_checks()[&tool].ok);
            match tool {
                SetupTool::Hlae => {
                    expected.hlae_exe = None;
                    assert!(paths.hlae_exe.unwrap().starts_with(engine.tools_dir()));
                }
                SetupTool::Ffmpeg => {
                    expected.ffmpeg_exe = None;
                    assert!(paths.ffmpeg_exe.unwrap().starts_with(engine.tools_dir()));
                }
                SetupTool::Vrf => {
                    expected.vrf_exe = None;
                    assert_eq!(paths.vrf_exe, Some(vrf.clone()));
                }
            }
            assert_eq!(
                serde_json::to_value(engine.settings()).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
        }
    }

    fn terminal_events(receiver: &mpsc::Receiver<Event>, count: usize) -> Vec<DemoMeta> {
        let mut finished = vec![];
        let mut running = HashSet::new();
        let mut validating = HashSet::new();
        while finished.len() < count {
            if let Event::DemoChanged { demo } =
                receiver.recv_timeout(Duration::from_secs(10)).unwrap()
            {
                match demo.status {
                    DemoStatus::Validating => {
                        assert!(validating.insert(demo.id.clone()), "duplicate validation");
                        assert!(running.insert(demo.id.clone()), "duplicate worker");
                        assert!(running.len() <= PARSE_CONCURRENCY);
                    }
                    DemoStatus::Parsing => {
                        if !validating.remove(&demo.id) {
                            assert!(running.insert(demo.id.clone()));
                        }
                        assert!(
                            running.len() <= PARSE_CONCURRENCY,
                            "automatic parsing exceeded its limit"
                        );
                    }
                    DemoStatus::Error | DemoStatus::Parsed => {
                        assert!(running.remove(&demo.id));
                        finished.push(demo);
                    }
                    _ => {}
                }
            }
        }
        finished
    }

    #[test]
    fn list_jobs_filters_deleted_videos_without_changing_records() {
        let temp = tempfile::tempdir().unwrap();
        let (send, _receive) = mpsc::channel();
        let engine = Engine::new(temp.path().join("data"), Arc::new(Events(send))).unwrap();
        let mut job = engine
            .store
            .new_job("demo", vec![], RenderOptions::default())
            .unwrap();
        job.status = JobStatus::Done;
        for name in ["clip.mp4", "merged.mp4"] {
            let file = engine.store.job_dir(&job.id).join(name);
            std::fs::write(&file, b"video").unwrap();
            job.outputs.push(JobOutput {
                file: file.to_string_lossy().into_owned(),
                bytes: 5,
                highlight_id: None,
                title: name.into(),
                is_final: name == "merged.mp4",
            });
        }
        engine.store.save_job(&job).unwrap();
        let record = engine.store.job_dir(&job.id).join("job.json");
        let original = std::fs::read(&record).unwrap();
        assert_eq!(engine.list_jobs().unwrap()[0].outputs.len(), 2);
        std::fs::remove_file(&job.outputs[0].file).unwrap();
        let listed = engine.list_jobs().unwrap();
        assert_eq!(listed[0].outputs.len(), 1);
        assert!(listed[0].outputs[0].is_final);
        std::fs::remove_file(&job.outputs[1].file).unwrap();
        let listed = engine.list_jobs().unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].outputs.is_empty());
        assert_eq!(listed[0].status, JobStatus::Done);
        std::fs::write(&job.outputs[0].file, b"restored").unwrap();
        assert_eq!(engine.list_jobs().unwrap()[0].outputs.len(), 1);
        assert_eq!(std::fs::read(record).unwrap(), original);
    }

    #[test]
    fn list_jobs_reports_video_check_errors_without_changing_records() {
        let temp = tempfile::tempdir().unwrap();
        let (send, _receive) = mpsc::channel();
        let engine = Engine::new(temp.path().join("data"), Arc::new(Events(send))).unwrap();
        let mut job = engine
            .store
            .new_job("demo", vec![], RenderOptions::default())
            .unwrap();
        let file = engine.store.job_dir(&job.id).join("invalid\0.mp4");
        job.outputs.push(JobOutput {
            file: file.to_string_lossy().into_owned(),
            bytes: 5,
            highlight_id: None,
            title: "invalid".into(),
            is_final: false,
        });
        engine.store.save_job(&job).unwrap();
        let record = engine.store.job_dir(&job.id).join("job.json");
        let original = std::fs::read(&record).unwrap();
        assert!(engine
            .list_jobs()
            .unwrap_err()
            .to_string()
            .contains("Cannot check video"));
        assert_eq!(std::fs::read(record).unwrap(), original);
    }

    #[test]
    fn completing_job_keeps_deletion_locked_until_result_is_persisted() {
        struct ObserveCompletion(Mutex<Option<std::sync::Weak<Engine>>>);
        impl Notify for ObserveCompletion {
            fn notify(&self, event: Event) {
                if let Event::JobChanged { job } = event {
                    if job.status == JobStatus::Error {
                        let engine = self.0.lock().unwrap().as_ref().unwrap().upgrade().unwrap();
                        assert_eq!(engine.active_job_id(), Some(job.id.clone()));
                        assert!(engine.data_lock.try_lock().is_err());
                        assert_eq!(
                            engine.store.get_job(&job.id).unwrap().status,
                            JobStatus::Error
                        );
                    }
                }
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let notify = Arc::new(ObserveCompletion(Mutex::new(None)));
        let engine = Engine::new(temp.path().join("data"), notify.clone()).unwrap();
        *notify.0.lock().unwrap() = Some(Arc::downgrade(&engine));
        let job = engine
            .store
            .new_job("missing-demo", vec![], RenderOptions::default())
            .unwrap();
        engine.run_job(job.clone());
        assert!(engine.active_job_id().is_none());
        engine.delete_job(&job.id).unwrap();
        assert!(engine.store.get_job(&job.id).is_none());
    }

    #[test]
    fn cleanup_clears_anomalies_and_owned_data_with_correct_scope() {
        for mode in 0..4 {
            let temp = tempfile::tempdir().unwrap();
            let data = temp.path().join("data");
            let store = Store::open(data.clone()).unwrap();
            store
                .save_settings(&Settings {
                    scan_game_replays: false,
                    ..Settings::default()
                })
                .unwrap();
            let source = temp.path().join("source.dem");
            std::fs::write(&source, b"demo").unwrap();
            let id = Engine::demo_id(&source);
            store.write_parse_error(&id, "manual retry").unwrap();
            let (send, _receive) = mpsc::channel();
            let engine = Engine::new(data.clone(), Arc::new(Events(send))).unwrap();
            engine.add_demo(&source).unwrap();
            std::fs::write(data.join("parsed").join(format!("{id}.json")), b"parsed").unwrap();
            std::fs::write(store.replay_path(&id), b"replay").unwrap();
            let job = store
                .new_job(&id, vec![], RenderOptions::default())
                .unwrap();
            let other = store
                .new_job("other", vec![], RenderOptions::default())
                .unwrap();
            crate::scoring::history::track_source(&data, &id, "sha1:owned").unwrap();
            crate::scoring::history::track_source(&data, "other", "sha1:other").unwrap();
            let states = data.join("analysis/match-state");
            std::fs::create_dir_all(&states).unwrap();
            let owned = states.join(format!(
                "{}-v7-test.gz",
                sha1_smol::Sha1::from("sha1:owned").digest()
            ));
            let untouched = states.join(format!(
                "{}-v7-test.gz",
                sha1_smol::Sha1::from("sha1:other").digest()
            ));
            std::fs::write(&owned, b"state").unwrap();
            std::fs::write(&untouched, b"state").unwrap();
            let clean = || match mode {
                0 => engine.clear_analysis(&id),
                1 => engine.remove_demo(&id),
                2 => engine.clear_match_anomaly(&id),
                _ => engine.clear_all_analysis().map(|_| ()),
            };
            engine.analysis_queue.lock().unwrap().enqueue(&id, false);
            assert!(clean().is_err());
            assert!(source.exists() && owned.exists() && store.job_dir(&job.id).exists());
            engine.analysis_queue.lock().unwrap().jobs.clear();
            {
                let _guard = engine.scoring_lock.lock().unwrap();
                assert!(clean().is_err());
            }
            clean().unwrap();
            assert_eq!(source.exists(), mode != 1);
            assert!(!owned.exists());
            assert_eq!(untouched.exists(), mode != 3);
            assert_eq!(store.job_dir(&job.id).exists(), mode == 2);
            assert_eq!(store.job_dir(&other.id).exists(), mode != 3);
            assert_eq!(
                data.join("parsed").join(format!("{id}.json")).exists(),
                mode == 2
            );
            assert_eq!(store.replay_path(&id).exists(), mode == 2);
            let match_dir = data
                .join("behavior-analysis/matches")
                .join(sha1_smol::Sha1::from(id.as_str()).digest().to_string());
            assert!(!match_dir.exists());
        }
    }

    #[test]
    fn removing_demo_cleans_owned_data_and_queued_jobs_but_preserves_shared_files() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let store = Store::open(data.clone()).unwrap();
        store
            .save_settings(&Settings {
                scan_game_replays: false,
                ..Settings::default()
            })
            .unwrap();
        let source = temp.path().join("remove.dem");
        std::fs::write(&source, b"demo").unwrap();
        let id = Engine::demo_id(&source);
        store.write_parse_error(&id, "manual retry").unwrap();
        let (send, _receive) = mpsc::channel();
        let engine = Engine::new(data.clone(), Arc::new(Events(send))).unwrap();
        engine.add_demo(&source).unwrap();
        let queued = store
            .new_job(&id, vec!["queued".into()], RenderOptions::default())
            .unwrap();
        let mut done = store
            .new_job(&id, vec!["done".into()], RenderOptions::default())
            .unwrap();
        done.status = JobStatus::Done;
        store.save_job(&done).unwrap();
        let other = store
            .new_job("another-demo", vec![], RenderOptions::default())
            .unwrap();
        for job in [&queued, &done, &other] {
            std::fs::write(store.job_dir(&job.id).join("video.mp4"), b"video").unwrap();
        }
        for suffix in ["json", "summary.json", "replay.json"] {
            std::fs::write(
                data.join("parsed").join(format!("{id}.{suffix}")),
                b"analysis",
            )
            .unwrap();
        }
        let radar = store.radar_dir().join("shared.png");
        std::fs::create_dir_all(store.radar_dir()).unwrap();
        std::fs::write(&radar, b"radar").unwrap();
        engine
            .render_queue
            .lock()
            .unwrap()
            .extend([queued.id.clone(), other.id.clone()]);
        engine.parsing.lock().unwrap().active.insert(id.clone());
        assert!(engine.remove_demo(&id).is_err());
        engine.parsing.lock().unwrap().active.clear();
        *engine.active_job.lock().unwrap() =
            Some((queued.id.clone(), Arc::new(AtomicBool::new(false))));
        assert!(engine.remove_demo(&id).is_err());
        assert!(source.exists() && store.job_dir(&done.id).exists());
        *engine.active_job.lock().unwrap() = None;
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let handles: Vec<_> = [&queued, &done]
                .iter()
                .map(|job| {
                    std::fs::OpenOptions::new()
                        .read(true)
                        .share_mode(1)
                        .open(store.job_dir(&job.id).join("job.json"))
                        .unwrap()
                })
                .collect();
            let failed = engine.remove_demo(&id);
            drop(handles);
            assert!(failed.is_err());
            assert!(source.exists());
            assert!(engine.render_queue.lock().unwrap().contains(&queued.id));
            assert_eq!(store.get_job(&queued.id).unwrap().status, JobStatus::Queued);
        }
        engine.remove_demo(&id).unwrap();
        assert!(!source.exists());
        assert!(engine.get_demo(&id).is_none());
        assert!(store.registered_demos().unwrap().is_empty());
        assert!(!store.job_dir(&done.id).exists());
        assert!(!store.job_dir(&queued.id).exists());
        assert!(store.job_dir(&other.id).join("video.mp4").exists());
        assert_eq!(
            engine
                .render_queue
                .lock()
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![&other.id]
        );
        assert!(!std::fs::read_dir(data.join("parsed")).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(&id)));
        assert_eq!(std::fs::read(radar).unwrap(), b"radar");
        engine.run_job(queued.clone()); // A worker may have dequeued it before removal.
        assert!(!store.job_dir(&queued.id).exists());
        assert!(store.delete_job("..").is_err());
        assert!(source.parent().unwrap().exists());
    }

    #[test]
    fn registered_same_named_demos_stay_in_place_and_survive_restart() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let first = temp.path().join("first/match.dem");
        let second = temp.path().join("second/match.dem");
        let store = Store::open(data.clone()).unwrap();
        store
            .save_settings(&Settings {
                scan_game_replays: false,
                cs2_dir: Some(temp.path().join("no-game").to_string_lossy().into_owned()),
                ..Settings::default()
            })
            .unwrap();
        for (path, bytes) in [(&first, "first source"), (&second, "second source")] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
            // Keep this registration test independent of the parser worker.
            store
                .write_parse_error(&Engine::demo_id(path), bytes)
                .unwrap();
        }
        let sibling = first.with_file_name("unselected.dem");
        std::fs::write(&sibling, "unselected").unwrap();
        let (send, _) = mpsc::channel();
        let engine = Engine::new(data.clone(), Arc::new(Events(send))).unwrap();
        let a = engine.add_demo(&first).unwrap();
        let b = engine.add_demo(&second).unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(Path::new(&a.path), first);
        assert_eq!(Path::new(&b.path), second);
        assert_eq!(engine.add_demo(&first).unwrap().id, a.id);
        assert_eq!(engine.list_demos().len(), 2);
        assert_eq!(
            store.registered_demos().unwrap(),
            vec![first.clone(), second.clone()]
        );
        assert!(engine.settings().replay_folders.is_empty());
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "first source");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "second source");
        assert_eq!(
            std::fs::read_dir(first.parent().unwrap()).unwrap().count(),
            2
        );
        assert_eq!(
            std::fs::read_dir(second.parent().unwrap()).unwrap().count(),
            1
        );
        assert!(!data.join("match.dem").exists());
        drop(engine);
        let (send, _) = mpsc::channel();
        let restarted = Engine::new(data, Arc::new(Events(send))).unwrap();
        let listed = restarted.list_demos();
        assert_eq!(listed.len(), 2);
        assert!(listed
            .iter()
            .any(|demo| demo.id == a.id && demo.error.as_deref() == Some("first source")));
        assert!(listed
            .iter()
            .any(|demo| demo.id == b.id && demo.error.as_deref() == Some("second source")));
    }

    fn complete_invalid_demo() -> Vec<u8> {
        let mut bytes = b"PBDEMS2\0".to_vec();
        bytes.extend_from_slice(&[0; 8]);
        bytes.extend_from_slice(&[0, 0, 0]);
        bytes
    }

    #[test]
    fn shared_parse_queue_refills_three_workers_while_replay_is_busy() {
        struct GatedEvents {
            events: mpsc::Sender<Event>,
            finished: mpsc::Sender<()>,
            gate: Arc<(Mutex<usize>, std::sync::Condvar)>,
        }
        impl Notify for GatedEvents {
            fn notify(&self, event: Event) {
                if matches!(&event, Event::DemoChanged { demo } if demo.status == DemoStatus::Error)
                {
                    let _ = self.finished.send(());
                    let (lock, ready) = &*self.gate;
                    // Hold completed workers' slots until the test releases them; no timing race.
                    let (mut permits, timeout) = ready
                        .wait_timeout_while(
                            lock.lock().unwrap(),
                            Duration::from_secs(10),
                            |permits| *permits == 0,
                        )
                        .unwrap();
                    assert!(!timeout.timed_out(), "worker was not released");
                    *permits -= 1;
                }
                let _ = self.events.send(event);
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let demos = temp.path().join("demos");
        std::fs::create_dir(&demos).unwrap();
        for i in 0..5 {
            std::fs::write(demos.join(format!("{i}.dem")), complete_invalid_demo()).unwrap();
        }
        Store::open(data.clone())
            .unwrap()
            .save_settings(&Settings {
                scan_game_replays: false,
                replay_folders: vec![demos.to_string_lossy().into_owned()],
                ..Default::default()
            })
            .unwrap();
        let (send, receive) = mpsc::channel();
        let (finished, finished_receive) = mpsc::channel();
        let gate = Arc::new((Mutex::new(0), std::sync::Condvar::new()));
        let engine = Engine::new(
            data,
            Arc::new(GatedEvents {
                events: send,
                finished,
                gate: gate.clone(),
            }),
        )
        .unwrap();
        // Keep the independent 2D worker busy for the entire parsing test.
        let (replay_reply, _) = mpsc::channel();
        {
            let mut queue = engine.replay_queue.lock().unwrap();
            queue.running = true;
            queue.pending.push_back(ReplayRequest {
                id: "replay".into(),
                replies: vec![replay_reply],
            });
        }
        let (replay_started, replay_ready) = mpsc::channel();
        let (release_replay, replay_release) = mpsc::channel();
        let replay_engine = engine.clone();
        let replay_worker = std::thread::spawn(move || {
            replay_engine.run_replay_queue(|_| {
                replay_started.send(()).unwrap();
                replay_release
                    .recv_timeout(Duration::from_secs(15))
                    .unwrap();
                Ok(PathBuf::from("replay.json"))
            })
        });
        replay_ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let (listed_send, listed_receive) = mpsc::channel();
        let scanner = engine.clone();
        let scan = std::thread::spawn(move || {
            listed_send.send(scanner.list_demos()).unwrap();
        });
        let listed = listed_receive.recv_timeout(Duration::from_secs(2));
        scan.join().unwrap();
        let listed = listed.expect("scanning must not wait for parsing or replay generation");
        assert_eq!(listed.len(), 5);
        for _ in 0..3 {
            finished_receive
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }
        assert_eq!(engine.parsing.lock().unwrap().active.len(), 3);
        let manual_source = temp.path().join("manual.dem");
        std::fs::write(&manual_source, complete_invalid_demo()).unwrap();
        let manual_id = Engine::demo_id(&manual_source);
        engine
            .store
            .write_parse_error(&manual_id, "manual retry")
            .unwrap();
        engine.add_demo(&manual_source).unwrap();
        engine.parse_demo(&manual_id).unwrap();
        for demo in &listed {
            engine.parse_demo(&demo.id).unwrap();
        }
        engine.parse_demo(&manual_id).unwrap();
        assert_eq!(engine.parsing.lock().unwrap().workers, 3);
        assert_eq!(engine.parsing.lock().unwrap().pending.len(), 3);
        assert_eq!(
            engine.get_demo(&manual_id).unwrap().0.status,
            DemoStatus::Queued
        );

        std::thread::scope(|scope| {
            for _ in 0..3 {
                scope.spawn(|| {
                    engine.list_demos();
                });
            }
        });
        assert_eq!(engine.parsing.lock().unwrap().active.len(), 3);
        assert_eq!(
            engine
                .demos
                .lock()
                .unwrap()
                .values()
                .filter(|e| e.meta.status == DemoStatus::Queued)
                .count(),
            3
        );
        // Release only one worker: its replacement must start before the other two finish.
        *gate.0.lock().unwrap() = 1;
        gate.1.notify_one();
        finished_receive
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(engine.parsing.lock().unwrap().active.len(), 3);
        assert_eq!(
            engine
                .demos
                .lock()
                .unwrap()
                .values()
                .filter(|entry| entry.meta.status == DemoStatus::Queued)
                .count(),
            2
        );
        *gate.0.lock().unwrap() = usize::MAX;
        gate.1.notify_all();
        let completed = terminal_events(&receive, 6);
        assert_eq!(
            completed
                .iter()
                .map(|d| &d.id)
                .collect::<HashSet<_>>()
                .len(),
            6
        );
        assert!(completed.iter().all(|d| d.status == DemoStatus::Error));
        assert!(engine.replay_queue.lock().unwrap().active.is_some());
        release_replay.send(()).unwrap();
        replay_worker.join().unwrap();
    }

    #[test]
    fn replay_publication_rejects_results_invalidated_during_build() {
        use serde_json::json;
        let temp = tempfile::tempdir().unwrap();
        let (send, _) = mpsc::channel();
        let engine = Engine::new(temp.path().join("data"), Arc::new(Events(send))).unwrap();
        let (_, info) = crate::scoring::clips::tests::fixture();
        let parsed = Arc::new(
            serde_json::from_value::<ParsedDemo>(json!({
                "info": info, "rounds": [], "kills": [], "highlights": [], "stats": [],
                "score": {}, "roundSummaries": [], "parsedAt": "same timestamp"
            }))
            .unwrap(),
        );
        let meta = serde_json::from_value(json!({
            "id": "demo", "name": "fixture.dem", "path": "fixture.dem",
            "bytes": 0, "mtimeMs": 0, "createdMs": 0, "status": "parsed"
        }))
        .unwrap();
        let entry = DemoEntry {
            meta,
            parsed: Some(parsed.clone()),
            auto_complete: None,
        };
        let replay = serde_json::from_value(json!({
            "schemaVersion": crate::replay::REPLAY_SCHEMA_VERSION, "tickRate": 64,
            "step": 4, "firstTick": 0, "lastTick": 0, "players": [], "weapons": [],
            "frames": [], "events": []
        }))
        .unwrap();
        for scenario in 0..4 {
            engine
                .demos
                .lock()
                .unwrap()
                .insert("demo".into(), entry.clone());
            let staged = engine.store.stage_replay(&replay).unwrap();
            let staged_path = staged.path().to_path_buf();
            engine.store.prune_parsed(&HashSet::new());
            assert!(
                staged_path.exists(),
                "rescanning must not remove staged replay data"
            );
            match scenario {
                1 => {
                    engine.update_meta("demo", DemoEntry::reset);
                }
                2 => {
                    engine.update_meta("demo", |e| e.parsed = Some(Arc::new((*parsed).clone())));
                }
                3 => {
                    engine.demos.lock().unwrap().remove("demo");
                }
                _ => {}
            }
            let result = engine.publish_replay("demo", &parsed, staged);
            assert_eq!(result.is_ok(), scenario == 0);
            assert_eq!(engine.store.replay_path("demo").exists(), scenario == 0);
            assert!(
                !staged_path.exists(),
                "staged data must be consumed or discarded"
            );
        }
    }

    #[test]
    fn replay_queue_is_fifo_coalesces_requests_and_continues_after_failure() {
        let temp = tempfile::tempdir().unwrap();
        let (send, _) = mpsc::channel();
        let engine = Engine::new(temp.path().join("data"), Arc::new(Events(send))).unwrap();
        // Start the worker explicitly so pending and active coalescing are deterministic.
        engine.replay_queue.lock().unwrap().running = true;
        let request = |id: &'static str| {
            let engine = engine.clone();
            std::thread::spawn(move || engine.replay_file(id))
        };
        let wait_for = |condition: &dyn Fn(&ReplayQueue) -> bool| {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !condition(&engine.replay_queue.lock().unwrap()) {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        let a = request("a");
        wait_for(&|q| q.pending.len() == 1);
        let a2 = request("a");
        wait_for(&|q| q.pending[0].replies.len() == 2);
        let b = request("b");
        wait_for(&|q| q.pending.len() == 2);
        let (started, start) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let worker_engine = engine.clone();
        let worker = std::thread::spawn(move || {
            let mut order = vec![];
            worker_engine.run_replay_queue(|id| {
                order.push(id.to_owned());
                if id == "a" {
                    started.send(()).unwrap();
                    released.recv_timeout(Duration::from_secs(5)).unwrap();
                    return Err(anyhow!("fixture build failure"));
                }
                Ok(PathBuf::from("b.replay.json"))
            });
            order
        });
        start.recv_timeout(Duration::from_secs(5)).unwrap();
        let a3 = request("a");
        wait_for(&|q| q.active.as_ref().is_some_and(|job| job.replies.len() == 3));
        release.send(()).unwrap();
        for reply in [a, a2, a3] {
            assert!(reply
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("fixture build failure"));
        }
        assert_eq!(b.join().unwrap().unwrap(), PathBuf::from("b.replay.json"));
        assert_eq!(worker.join().unwrap(), ["a", "b"]);
        assert!(!engine.replay_queue.lock().unwrap().running);
    }

    #[test]
    fn automatic_parse_waits_for_complete_source() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let source = temp.path().join("match.dem");
        let bytes = complete_invalid_demo();
        std::fs::write(&source, &bytes[..16]).unwrap();
        let store = Store::open(data.clone()).unwrap();
        store
            .save_settings(&Settings {
                scan_game_replays: false,
                ..Settings::default()
            })
            .unwrap();
        let (send, receive) = mpsc::channel();
        let engine = Engine::new(data, Arc::new(Events(send))).unwrap();
        let demo = engine.add_demo(&source).unwrap();
        let wait_skipped = || {
            let mut validating = false;
            loop {
                if let Event::DemoChanged { demo } =
                    receive.recv_timeout(Duration::from_secs(5)).unwrap()
                {
                    match demo.status {
                        DemoStatus::Queued => {}
                        DemoStatus::Validating => validating = true,
                        DemoStatus::New => {
                            assert!(validating);
                            break;
                        }
                        _ => panic!("incomplete or writable file must not be parsed"),
                    }
                }
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while engine.is_parsing(&demo.id) {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(store.parse_error(&demo.id).is_none());
        };
        wait_skipped();
        assert_eq!(
            engine.demos.lock().unwrap()[&demo.id].auto_complete,
            Some(false)
        );
        std::fs::write(&source, &bytes).unwrap();
        #[cfg(windows)]
        {
            let writer = std::fs::OpenOptions::new()
                .append(true)
                .open(&source)
                .unwrap();
            engine.list_demos();
            wait_skipped();
            drop(writer);
        }
        engine.list_demos();
        // Complete framing is ready immediately; invalid game data still fails once.
        let completed = terminal_events(&receive, 1);
        assert_eq!(completed[0].status, DemoStatus::Error);
        assert!(store.parse_error(&demo.id).is_some());
    }

    #[test]
    fn auto_parse_failure_is_not_retried_until_manual_parse() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let demos = temp.path().join("demos");
        std::fs::create_dir_all(&demos).unwrap();
        for name in ["first.dem", "second.dem"] {
            std::fs::write(demos.join(name), complete_invalid_demo()).unwrap();
        }
        Store::open(data.clone())
            .unwrap()
            .save_settings(&Settings {
                scan_game_replays: false,
                replay_folders: vec![demos.to_string_lossy().into_owned()],
                cs2_dir: Some(temp.path().join("no-game").to_string_lossy().into_owned()),
                ..Settings::default()
            })
            .unwrap();
        let (send, receive) = mpsc::channel();
        let engine = Engine::new(data.clone(), Arc::new(Events(send))).unwrap();
        assert_eq!(engine.list_demos().len(), 2);
        let completed = terminal_events(&receive, 2);
        assert!(completed.iter().all(|d| d.status == DemoStatus::Error));
        // Scans, changed file contents and clearing cached analysis do not retry errors.
        std::fs::write(&completed[0].path, b"still not a demo, but changed").unwrap();
        engine.clear_all_analysis().unwrap();
        assert!(engine
            .list_demos()
            .iter()
            .all(|d| d.status == DemoStatus::Error));
        assert!(receive.try_iter().all(
            |e| !matches!(e, Event::DemoChanged { demo } if demo.status == DemoStatus::Parsing)
        ));
        let settings = engine.settings();
        let mut disconnected = settings.clone();
        disconnected.replay_folders.clear();
        engine.store.save_settings(&disconnected).unwrap();
        assert!(engine.list_demos().is_empty());
        engine.store.save_settings(&settings).unwrap();
        assert!(engine
            .list_demos()
            .iter()
            .all(|d| d.status == DemoStatus::Error));
        drop(engine);
        let (send, receive) = mpsc::channel();
        let restarted = Engine::new(data, Arc::new(Events(send))).unwrap();
        assert!(restarted
            .list_demos()
            .iter()
            .all(|d| d.status == DemoStatus::Error));
        assert!(receive.try_recv().is_err());
        // A failed reparse must not leave old statistics or a reusable 2D cache.
        let id = &completed[0].id;
        let cache_dir = restarted
            .store
            .replay_path(id)
            .parent()
            .unwrap()
            .to_path_buf();
        let caches: Vec<_> = ["json", "summary.json", "replay.json"]
            .iter()
            .map(|suffix| cache_dir.join(format!("{id}.{suffix}")))
            .collect();
        for path in &caches {
            std::fs::write(path, b"old cached data").unwrap();
        }
        let other_replay = restarted.store.replay_path(&completed[1].id);
        std::fs::write(&other_replay, b"other demo").unwrap();
        let source_before = std::fs::read(&completed[0].path).unwrap();
        restarted.parse_demo(&completed[0].id).unwrap();
        let retried = terminal_events(&receive, 1);
        assert_eq!(retried[0].id, completed[0].id);
        assert_eq!(retried[0].status, DemoStatus::Error);
        assert!(caches.iter().all(|path| !path.exists()));
        assert!(restarted.parsed(id).is_none());
        assert_eq!(std::fs::read(&other_replay).unwrap(), b"other demo");
        assert_eq!(std::fs::read(&completed[0].path).unwrap(), source_before);
    }
}

#[cfg(test)]
mod analysis_queue_tests {
    use super::*;
    #[derive(Default)]
    struct Events(Mutex<Vec<(String, crate::scoring::queue::Status)>>);
    impl Notify for Events {
        fn notify(&self, event: Event) {
            if let Event::AnalysisJobChanged { job } = event {
                self.0.lock().unwrap().push((job.demo_id, job.status));
            }
        }
    }
    #[test]
    fn interrupted_render_persists_language_independent_error_code() {
        let root = tempfile::tempdir().unwrap();
        let engine = Engine::new(root.path().into(), Arc::new(Events::default())).unwrap();
        let mut job = engine
            .store
            .new_job("demo", vec!["clip".into()], RenderOptions::default())
            .unwrap();
        job.status = JobStatus::Running;
        engine.store.save_job(&job).unwrap();
        let reopened = Engine::new(root.path().into(), Arc::new(Events::default())).unwrap();
        let job = reopened.store.get_job(&job.id).unwrap();
        assert_eq!(job.status, JobStatus::Error);
        assert_eq!(job.error_code, Some(crate::ErrorCode::AppClosed));
        assert_eq!(
            serde_json::to_value(&job).unwrap()["errorCode"],
            "app-closed"
        );
        assert!(job.error.is_some());
    }

    #[test]
    fn queued_render_survives_restart_and_queue_pause_is_persisted() {
        let root = tempfile::tempdir().unwrap();
        let engine = Engine::new(root.path().into(), Arc::new(Events::default())).unwrap();
        let queued = engine
            .store
            .new_job("demo", vec!["clip".into()], RenderOptions::default())
            .unwrap();
        std::fs::write(root.path().join("queue-paused"), b"paused").unwrap();
        let reopened = Engine::new(root.path().into(), Arc::new(Events::default())).unwrap();
        assert!(reopened.queue_paused());
        assert_eq!(
            reopened.store.get_job(&queued.id).unwrap().status,
            JobStatus::Queued
        );
        assert_eq!(
            *reopened.render_queue.lock().unwrap(),
            VecDeque::from([queued.id])
        );
    }

    #[test]
    fn anomaly_storage_counts_both_folders_and_clears_only_when_idle() {
        let root = tempfile::tempdir().unwrap();
        let engine = Engine::new(root.path().into(), Arc::new(Events::default())).unwrap();
        for name in ["analysis", "behavior-analysis", "parsed", "clips"] {
            std::fs::create_dir_all(root.path().join(name)).unwrap();
            std::fs::write(root.path().join(name).join("keep-or-clear"), b"123").unwrap();
        }
        assert_eq!(engine.storage_bytes().3, 6);
        {
            let _guard = engine.scoring_lock.lock().unwrap();
            assert!(engine.clear_anomaly_data().is_err());
        }
        engine.analysis_queue.lock().unwrap().enqueue("demo", true);
        assert!(engine.clear_anomaly_data().is_err());
        assert_eq!(engine.storage_bytes().3, 6);
        engine.analysis_queue.lock().unwrap().jobs.clear();
        assert_eq!(engine.clear_anomaly_data().unwrap(), 6);
        assert_eq!(engine.storage_bytes().3, 0);
        assert_eq!(engine.clear_anomaly_data().unwrap(), 0);
        for name in ["parsed", "clips"] {
            assert!(root.path().join(name).join("keep-or-clear").exists());
        }
    }

    #[test]
    fn anomaly_exports_share_highlight_fifo_and_force_one_video_per_rule() {
        use serde_json::json;
        let root = tempfile::tempdir().unwrap();
        let engine = Engine::new(root.path().into(), Arc::new(Events::default())).unwrap();
        // Hold the worker without launching CS2; inspect the shared queue and saved jobs.
        engine.render_worker_running.store(true, Ordering::SeqCst);
        let source = root.path().join("test.dem");
        std::fs::write(&source, b"FIFO source fixture").unwrap();
        let (mut record, info) = crate::scoring::clips::tests::fixture();
        record.demo_fingerprint = format!(
            "sha1:{}",
            sha1_smol::Sha1::from(b"FIFO source fixture").digest()
        );
        let mut records = vec![record];
        crate::scoring::history::save_match(root.path(), &mut records).unwrap();
        let parsed:ParsedDemo=serde_json::from_value(json!({"info":info,"rounds":[{"round":1,"startTick":0,"freezeEndTick":0,"endTick":6400,"officiallyEndedTick":6400,"reason":"","roster":{}}],"kills":[],"highlights":[{"id":"highlight","player":{"steamid":"1","name":"Fixture"},"round":1,"startTick":100,"endTick":200,"anchorTick":150,"score":1,"tags":[],"title":"Fixture highlight","kills":[],"breakdown":{}}],"stats":[],"score":{},"roundSummaries":[],"parsedAt":""})).unwrap();
        let meta=serde_json::from_value(json!({"id":"demo","name":"test.dem","path":source,"bytes":0,"mtimeMs":0,"createdMs":0,"status":"parsed"})).unwrap();
        engine.demos.lock().unwrap().insert(
            "demo".into(),
            DemoEntry {
                meta,
                parsed: Some(Arc::new(parsed)),
                auto_complete: None,
            },
        );
        let first = engine
            .enqueue_render("demo", vec!["highlight".into()], RenderOptions::default())
            .unwrap();
        let selection = crate::scoring::clips::Selection {
            player_id: "1".into(),
            assessment_id: records[0].id.clone(),
            rule_ids: vec!["jump".into(), "view".into()],
        };
        let jobs = engine
            .enqueue_analysis_render(
                "demo",
                selection.clone(),
                RenderOptions {
                    merge: false,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(
            *engine.render_queue.lock().unwrap(),
            VecDeque::from([first.id, jobs[0].id.clone(), jobs[1].id.clone()])
        );
        assert!(jobs
            .iter()
            .all(|j| j.options.merge && j.status == JobStatus::Queued));
        assert_eq!(
            engine
                .store
                .get_job(&jobs[0].id)
                .unwrap()
                .analysis_clips
                .unwrap()
                .highlights
                .len(),
            6
        );
        assert!(engine.cancel_job(&jobs[0].id));
        assert_eq!(
            engine.store.get_job(&jobs[0].id).unwrap().status,
            JobStatus::Cancelled
        );
        assert_eq!(engine.render_queue.lock().unwrap().len(), 2);
        let preview = engine.analysis_clips("demo", &selection, true).unwrap();
        let merged = engine
            .enqueue_analysis_render(
                "demo",
                selection,
                RenderOptions {
                    merge: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(merged.len(), 1);
        let snapshot = engine
            .store
            .get_job(&merged[0].id)
            .unwrap()
            .analysis_clips
            .unwrap();
        let expected = [
            (448, 1024),
            (1184, 1440),
            (1824, 2080),
            (2464, 2720),
            (3104, 3360),
            (3744, 4096),
        ];
        assert_eq!(
            snapshot
                .highlights
                .iter()
                .map(|h| (h.start_tick, h.end_tick))
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(snapshot.highlights[0].tags, ["jump", "view"]);
        assert_eq!(
            serde_json::to_value(&snapshot).unwrap(),
            serde_json::to_value(&preview[0]).unwrap()
        );
        assert_eq!(
            engine.render_queue.lock().unwrap().back(),
            Some(&merged[0].id)
        );
        assert!(merged[0].options.merge);
    }

    #[test]
    fn failed_job_does_not_block_next_and_queue_is_session_only() {
        use crate::scoring::queue::Status;
        let root = tempfile::tempdir().unwrap();
        let events = Arc::new(Events::default());
        let engine = Engine::new(root.path().into(), events.clone()).unwrap();
        {
            let mut q = engine.analysis_queue.lock().unwrap();
            q.enqueue("missing-a", true);
            q.enqueue("missing-b", true);
            q.worker_running = true;
        }
        engine.clone().run_analysis_queue();
        let jobs = engine.analysis_jobs();
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().all(|j| j.status == Status::Error
            && j.error.as_deref().unwrap().contains("demo not found")
            && j.finished_at.is_some()));
        let events = events.0.lock().unwrap();
        let first_failed = events
            .iter()
            .position(|(id, status)| id == "missing-a" && *status == Status::Error)
            .unwrap();
        let next_started = events
            .iter()
            .position(|(id, status)| id == "missing-b" && *status == Status::Running)
            .unwrap();
        assert!(first_failed < next_started);
        assert!(!engine.analysis_queue.lock().unwrap().worker_running);
        let queue_file = root.path().join("analysis-jobs.json");
        assert!(!queue_file.exists());
        let (record, _) = crate::scoring::clips::tests::fixture();
        crate::scoring::history::save_match(root.path(), &mut [record]).unwrap();
        std::fs::write(&queue_file, b"old queue is no longer read").unwrap();
        let reopened = Engine::new(root.path().into(), Arc::new(Events::default())).unwrap();
        assert!(reopened.analysis_jobs().is_empty());
        assert!(!queue_file.exists());
        assert_eq!(reopened.scoring_history("demo", "1").unwrap().len(), 1);
    }
}
