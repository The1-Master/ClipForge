//! Local automation bridge. Requests travel over stdin; owned process trees die on cancel/exit.
use crate::workspace;
use demodesk_core::{
    engine::{Engine, JobReservation},
    render::{process::ProcessTree, RenderOptions},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
const DEFERRED: &str = "\u{1d}clipforge-deferred:";
const REVIEW_PIPELINE_VERSION: &str =
    "clipforge-local-vision-v8-event-centered-targeted-multi-event-personal-death-camera-source-ordinal-pts-8s6s-2fps-32k";

pub struct Automation {
    engine: Arc<Engine>,
    cached: Mutex<Value>,
    activity: Mutex<Value>,
    current: Mutex<Option<Arc<AtomicBool>>>,
    dispatch: Mutex<()>,
    review: Mutex<Value>,
    review_files: Mutex<Option<JobReservation>>,
    stopped: AtomicBool,
}

impl Automation {
    pub fn new(engine: Arc<Engine>) -> Arc<Self> {
        let this = Arc::new(Self {
            engine,
            cached: Mutex::new(json!({})),
            activity: Mutex::new(json!({"busy":false})),
            current: Mutex::new(None),
            dispatch: Mutex::new(()),
            review: Mutex::new(
                std::fs::read(workspace::root().join("state/automation/review.json"))
                    .ok()
                    .filter(|v| v.len() <= 4 * 1024 * 1024)
                    .and_then(|v| serde_json::from_slice(&v).ok())
                    .unwrap_or(Value::Null),
            ),
            stopped: AtomicBool::new(false),
            review_files: Mutex::new(None),
        });
        let previous_review = this.review.lock().unwrap().clone();
        if previous_review["complete"] == false {
            if let Some(id) = previous_review["jobId"].as_str() {
                if let Ok(reservation) = this.engine.reserve_job(id) {
                    *this.review_files.lock().unwrap() = Some(reservation);
                }
            }
        }
        let background = this.clone();
        std::thread::spawn(move || {
            let _ = background.refresh();
            background.engine.list_demos();
            background.engine.resume_saved_queue();
            let mut next_sync = Instant::now() + Duration::from_secs(5);
            let mut last_game_open = false;
            let mut next_queue = Instant::now();
            while !background.stopped.load(Ordering::SeqCst) {
                let game_open = demodesk_core::render::cs2_is_running().unwrap_or(true);
                if last_game_open && !game_open {
                    next_sync = Instant::now();
                }
                last_game_open = game_open;
                let review = background.review.lock().unwrap().clone();
                if cancellation_cleaned_up(
                    &review,
                    background.busy(),
                    background.engine.active_job_id().as_deref(),
                ) {
                    // Finishing cancellation is local bookkeeping even when a user's game is open.
                    background.review_error("Whole-match review was cancelled.");
                }
                background.release_review_files();
                if !game_open && !background.busy() && background.engine.active_job_id().is_none() {
                    background.advance_review();
                }
                let (settings, match_history) = {
                    let cached = background.cached.lock().unwrap();
                    (cached["settings"].clone(), cached["capabilities"]["matchHistory"] != false)
                };
                if settings["enabled"].as_bool() == Some(true) && !game_open {
                    if match_history && Instant::now() >= next_sync
                        && !background.busy()
                        && background.engine.active_job_id().is_none()
                    {
                        let _ = background.start(json!({"command":"sync"}));
                        next_sync = Instant::now() + Duration::from_secs(300);
                    }
                    if Instant::now() >= next_queue {
                        background.queue_own_moments(&settings);
                        background.queue_ai_review(&settings);
                        next_queue = Instant::now() + Duration::from_secs(10);
                    }
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        });
        this
    }

    fn busy(&self) -> bool {
        self.current.lock().unwrap().is_some()
    }

    pub fn snapshot(&self) -> Value {
        let mut result = self.cached.lock().unwrap().clone();
        result["activity"] = self.activity.lock().unwrap().clone();
        result["queuePaused"] = json!(self.engine.queue_paused());
        let presence = demodesk_core::render::cs2_presence();
        result["gameOpen"] = json!(presence != "closed");
        result["gamePresence"] = json!(presence);
        let mut review = self.review.lock().unwrap().clone();
        if review["complete"] == false && review["cancelling"] != true {
            if presence == "external" || presence == "unknown" {
                review["phase"] = json!("waiting-for-game");
                review["status"] = json!(if presence == "external" {
                    "Waiting for CS2 to close"
                } else {
                    "Waiting until CS2 process status can be checked"
                });
            } else if review["jobId"]
                .as_str()
                .is_some_and(|id| self.engine.active_job_id().as_deref() == Some(id))
            {
                review["phase"] = json!("preview-recording");
                review["status"] = json!("Recording review previews");
            }
        }
        result["review"] = review;
        result
    }

    pub fn refresh(&self) -> Result<Value, String> {
        let data = self.call(
            json!({"command":"status"}),
            Arc::new(AtomicBool::new(false)),
            false,
        )?;
        *self.cached.lock().unwrap() = data;
        Ok(self.snapshot())
    }

    pub fn save(&self, values: Value) -> Result<Value, String> {
        let _dispatch = self.dispatch.lock().unwrap();
        if self.busy() || self.review.lock().unwrap()["complete"] == false {
            let identity_changed = ["steamId", "lastShareCode", "model", "aiThreshold"]
                .iter()
                .any(|key| values.get(*key).is_some());
            let secret_changed = ["apiKey", "authCode"]
                .iter()
                .any(|key| values[*key].as_str().is_some_and(|v| !v.is_empty()));
            if identity_changed || secret_changed {
                return Err("Finish or cancel the current task before changing the account, starting token, credentials, or AI model.".into());
            }
        }
        self.call(
            json!({"command":"save", "values":values}),
            Arc::new(AtomicBool::new(false)),
            false,
        )?;
        self.refresh()
    }

    pub fn start(self: &Arc<Self>, request: Value) -> Result<(), String> {
        let _dispatch = self.dispatch.lock().unwrap();
        let command = request["command"].as_str().unwrap_or("");
        if !matches!(command, "sync" | "install-models" | "review" | "retry") {
            return Err("Unknown automation action.".into());
        }
        if command == "review" {
            for field in ["video", "sidecar"] {
                let path = request[field]
                    .as_str()
                    .ok_or("Choose a video and its recording timeline.")?;
                let approved = workspace::owned_path(std::path::Path::new(path))?;
                if !approved.is_file() {
                    return Err("The review input is missing.".into());
                }
            }
        }
        let mut slot = self.current.lock().unwrap();
        if slot.is_some() {
            return Err("An automation task is already running.".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *slot = Some(cancel.clone());
        let demo_id = self.review.lock().unwrap()["demoId"].clone();
        *self.activity.lock().unwrap() = json!({"busy":true,"task":command,"phase":"starting","progress":"Starting…", "startedAt":demodesk_core::store::now(),"cancelling":false,"demoId":if command == "review" {demo_id} else {Value::Null}});
        let this = self.clone();
        std::thread::spawn(move || {
            let reviewing = request["command"] == "review";
            let outcome = this.call(request, cancel.clone(), true);
            let activity = match outcome {
                Ok(result) => {
                    if reviewing {
                        this.accept_review(&result);
                    }
                    if let Some(paths) = result["downloaded"].as_array() {
                        for path in paths.iter().filter_map(Value::as_str) {
                            if workspace::owned_path(std::path::Path::new(path)).is_ok() {
                                let _ = this.engine.add_demo(std::path::Path::new(path));
                            }
                        }
                    }
                    json!({"busy":false,"progress":"Finished","result":result})
                }
                Err(error) => {
                    if let Some(message) = error.strip_prefix(DEFERRED) {
                        let presence = demodesk_core::render::cs2_presence();
                        let phase = if presence == "external" || presence == "unknown" {
                            "waiting-for-game"
                        } else {
                            "waiting-for-compute"
                        };
                        let mut review = this.review.lock().unwrap();
                        if reviewing && review.is_object() && review["complete"] == false {
                            review["status"] = json!(message);
                            review["phase"] = json!(phase);
                            this.save_review(&review);
                        }
                        json!({"busy":false,"progress":message,"phase":phase})
                    } else {
                        if reviewing && !this.stopped.load(Ordering::SeqCst) {
                            this.review_error(&error);
                        }
                        json!({"busy":false,"error":error,
                    "progress":if cancel.load(Ordering::SeqCst) {"Cancelled"} else {"Needs attention"}})
                    }
                }
            };
            let mut old = this.activity.lock().unwrap();
            *old = finalize_activity(activity, &old, cancel.load(Ordering::SeqCst));
            drop(old);
            *this.current.lock().unwrap() = None;
            this.release_review_files();
            let _ = this.refresh();
        });
        Ok(())
    }

    pub fn cancel(&self) {
        if let Some(cancel) = self.current.lock().unwrap().as_ref() {
            cancel.store(true, Ordering::SeqCst);
            let mut activity = self.activity.lock().unwrap();
            activity["cancelling"] = json!(true);
            activity["phase"] = json!("cancelling");
            activity["progress"] = json!("Cancelling…");
        }
        let review = self.review.lock().unwrap().clone();
        if review["complete"] == false {
            if let Some(id) = review["jobId"].as_str() {
                self.engine.cancel_job(id);
            }
            let mut current = self.review.lock().unwrap();
            current["cancelling"] = json!(true);
            current["phase"] = json!("cancelling");
            current["status"] = json!("Cancelling…");
            self.save_review(&current);
        }
    }

    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(cancel) = self.current.lock().unwrap().as_ref() {
            cancel.store(true, Ordering::SeqCst);
        }
    }

    fn save_review(&self, review: &Value) {
        let directory = workspace::root().join("state/automation");
        if std::fs::create_dir_all(&directory).is_ok() {
            if let Ok(bytes) = serde_json::to_vec(review) {
                let temp = directory.join("review.tmp");
                if std::fs::write(&temp, bytes).is_ok() {
                    let _ = std::fs::rename(temp, directory.join("review.json"));
                }
            }
        }
    }

    fn release_review_files(&self) {
        if self.busy() {
            return;
        }
        let review = self.review.lock().unwrap().clone();
        if review["complete"] == true
            && review["jobId"].as_str() != self.engine.active_job_id().as_deref()
        {
            self.review_files.lock().unwrap().take();
        }
    }

    fn review_error(&self, message: &str) {
        let mut review = self.review.lock().unwrap();
        if review.is_object() {
            review["status"] = json!("Needs attention");
            review["error"] = json!(message);
            review["complete"] = json!(true);
            review["phase"] = json!(if review["cancelling"] == true {
                "cancelled"
            } else {
                "failed"
            });
            if review["cancelling"] == true {
                review["status"] = json!("Cancelled");
            }
            review["cancelling"] = json!(false);
            self.save_review(&review);
        }
    }

    fn review_marker(&self, digest: &str, player: &str, model: &str) -> std::path::PathBuf {
        let key = format!(
            "{:x}",
            Sha256::digest(format!(
                "{REVIEW_PIPELINE_VERSION}:{digest}:{player}:{model}"
            ))
        );
        workspace::root()
            .join("state/automation")
            .join(format!("reviewed-{key}"))
    }

    fn queue_ai_review(self: &Arc<Self>, settings: &Value) {
        if settings["aiEnabled"] != true || self.busy() {
            return;
        }
        let pending = self.review.lock().unwrap().clone();
        if pending.is_object() && pending["complete"] == false {
            return;
        }
        let Some(player) = settings["steamId"].as_str().filter(|v| !v.is_empty()) else {
            return;
        };
        let model = settings["model"].as_str().unwrap_or("qwen3-vl:8b-instruct");
        let cached = self.cached.lock().unwrap().clone();
        if !cached["models"]["models"].as_array().is_some_and(|models| {
            models
                .iter()
                .any(|m| m["name"].as_str() == Some(model) && m["installed"] == true)
        }) {
            return;
        }
        for meta in self.engine.list_demos() {
            let Some(parsed) = self.engine.parsed(&meta.id) else {
                continue;
            };
            if !parsed.info.players.iter().any(|p| p.steamid == player) {
                continue;
            }
            let marker = self.review_marker(&parsed.demo_digest, player, model);
            if marker.exists() {
                continue;
            }
            // One automatic attempt per source/model. An explicit manual review can retry.
            if self.prepare_review(meta.id, player.to_string()).is_ok() {
                let _ = std::fs::write(marker, b"queued");
                break;
            }
        }
    }

    pub fn prepare_review(
        self: &Arc<Self>,
        demo_id: String,
        player_id: String,
    ) -> Result<(), String> {
        let profile = self.call(json!({"command":"community","action":"profile_snapshot",
            "values":{"playerId":player_id}}), Arc::new(AtomicBool::new(false)), false)?;
        self.prepare_review_mode_with_profile(demo_id, player_id, "targeted".into(), profile)
    }

    pub fn prepare_review_mode(
        self: &Arc<Self>, demo_id: String, player_id: String, mode: String,
    ) -> Result<(), String> {
        self.prepare_review_mode_with_profile(demo_id, player_id, mode, json!({}))
    }

    pub fn prepare_review_mode_with_profile(
        self: &Arc<Self>, demo_id: String, player_id: String, mode: String, profile: Value,
    ) -> Result<(), String> {
        let _dispatch = self.dispatch.lock().unwrap();
        if self.busy() {
            return Err("Finish or cancel the current task first.".into());
        }
        let mut review = self.review.lock().unwrap();
        if review.is_object() && review["complete"] == false {
            return Err(
                "A whole-match review is already queued. Cancel it before starting another.".into(),
            );
        }
        let config = self.cached.lock().unwrap().clone();
        let model = config["settings"]["model"]
            .as_str()
            .unwrap_or("qwen3-vl:8b-instruct");
        let installed = config["models"]["models"].as_array().is_some_and(|models| {
            models.iter().any(|m| {
                m["name"].as_str() == Some(model) && m["installed"].as_bool() == Some(true)
            })
        });
        if !installed {
            return Err("Download the selected local AI model first.".into());
        }
        let model_digest = config["models"]["models"].as_array().into_iter().flatten().find(|m| m["name"].as_str() == Some(model)).map(|m| m["digest"].clone()).unwrap_or(Value::Null);
        let parsed = self
            .engine
            .parsed(&demo_id)
            .ok_or("Parse this match before reviewing it.")?;
        if !parsed.info.players.iter().any(|p| p.steamid == player_id) {
            return Err("Your saved Steam ID does not appear in this match.".into());
        }
        let plan = demodesk_core::detector::review_plan(&parsed, &player_id, &mode)?;
        if plan.windows.is_empty() {
            return Err("No event candidates were found. Choose Full visual sweep to inspect noncombat moments.".into());
        }
        if plan.windows.len() > 512 {
            return Err("This match has more than 512 review batches. Choose Full visual sweep or review a shorter demo.".into());
        }
        let windows: Vec<_> = plan.windows.iter().map(|w| [w.start_tick, w.end_tick]).collect();
        let profile_bytes = serde_json::to_vec(&profile).map_err(|_| "Invalid learning profile")?;
        if profile_bytes.len() > 16384 { return Err("The personal learning profile is too large.".into()); }
        let profile_hash = format!("{:x}", Sha256::digest(&profile_bytes));
        let mut options = RenderOptions::default();
        options.width = 1280;
        options.height = 720;
        options.fps = 15;
        options.merge = false;
        options.key_moments_only = false;
        options.voice = true;
        let (job, reservation) = self
            .engine
            .enqueue_review_preview(
                &demo_id,
                &player_id,
                windows,
                if mode == "targeted" { "Targeted AI preview" } else { "Full visual sweep preview" }.into(),
                options,
            )
            .map_err(|e| e.to_string())?;
        *self.review_files.lock().unwrap() = Some(reservation);
        let coverage: Vec<_> = parsed.rounds.iter().map(|r| json!({"startTick":r.freeze_end_tick,"endTick":r.officially_ended_tick,
            "round":r.round,"level":"not-reviewed","status":"not-reviewed"})).collect();
        *review = json!({"demoId":demo_id,"jobId":job.id,"playerId":player_id,"pipeline":REVIEW_PIPELINE_VERSION,
            "demoDigest":parsed.demo_digest,"status":"Preview queued", "moments":[],
            "nextOutput":0,"complete":false,"model":model,"modelDigest":model_digest,"threshold":config["settings"]["aiThreshold"].as_u64().unwrap_or(70),
            "mode":mode,"plan":plan,"coverage":coverage,"reviewedWindows":0,"unavailableWindows":0,
            "profileHash":profile_hash,"learningProfile":profile,"renderDefaults":self.engine.recording_defaults(),
            "autoExport":config["settings"]["enabled"] == true && config["settings"]["autoExport"] == true,
            "phase":"preview-queued","completed":0,"total":job.highlight_ids.len(),"cancelling":false,"startedAt":demodesk_core::store::now()});
        self.save_review(&review);
        Ok(())
    }

    fn advance_review(self: &Arc<Self>) {
        use demodesk_core::store::JobStatus;
        let review = self.review.lock().unwrap().clone();
        if !review.is_object() || review["complete"] != false {
            return;
        }
        if review["cancelling"] == true {
            // This function runs only after both owned preview and inference workers have cleaned up.
            self.review_error("Whole-match review was cancelled.");
            return;
        }
        let (Some(demo_id), Some(job_id), Some(player_id)) = (
            review["demoId"].as_str(),
            review["jobId"].as_str(),
            review["playerId"].as_str(),
        ) else {
            return;
        };
        let Some(parsed) = self.engine.parsed(demo_id) else {
            return;
        };
        if review["demoDigest"].as_str() != Some(parsed.demo_digest.as_str()) {
            self.review_error("This demo changed. Start a new review.");
            return;
        }
        if review["pipeline"].as_str() != Some(REVIEW_PIPELINE_VERSION) {
            self.review_error("The review pipeline changed. Start a new review; previous outputs were retained.");
            return;
        }
        let job = self
            .engine
            .list_jobs()
            .ok()
            .and_then(|jobs| jobs.into_iter().find(|j| j.id == job_id));
        let Some(job) = job else {
            self.review_error("The review preview job is missing.");
            return;
        };
        if matches!(job.status, JobStatus::Queued | JobStatus::Running) {
            return;
        }
        if !matches!(job.status, JobStatus::Done | JobStatus::Partial) {
            self.review_error(
                job.error
                    .as_deref()
                    .unwrap_or("The preview recording did not finish."),
            );
            return;
        }
        let outputs: Vec<_> = job
            .outputs
            .iter()
            .filter(|o| !o.is_final && o.highlight_id.is_some())
            .collect();
        let index = review["nextOutput"].as_u64().unwrap_or(0) as usize;
        if let Some(output) = outputs.get(index) {
            {
                let mut review = self.review.lock().unwrap();
                review["phase"] = json!("review");
                review["completed"] = json!(index);
                review["total"] = json!(outputs.len());
                self.save_review(&review);
            }
            let sidecar = format!("{}.timeline.json", output.file);
            if !std::path::Path::new(&sidecar).is_file() {
                self.review_error("The preview is missing its measured recording timeline. Re-record it before AI review.");
                return;
            }
            let highlight = job.source_snapshot.as_ref().and_then(|s| s.highlights.iter().find(|h| Some(h.id.as_str()) == output.highlight_id.as_deref()));
            let selected_candidates: Vec<_> = review["plan"]["candidates"].as_array().into_iter().flatten()
                .filter(|c| highlight.is_some_and(|h| c["startTick"].as_i64().is_some_and(|start| start <= h.end_tick as i64)
                    && c["endTick"].as_i64().is_some_and(|end| end >= h.start_tick as i64)))
                .map(|c| {
                    let mut compact = c.clone();
                    // Complete evidence remains in the native plan; one review
                    // batch sends a bounded factual overview over stdin.
                    if let Some(facts) = compact["evidence"].as_array() { compact["evidence"] = json!(facts.iter().take(12).collect::<Vec<_>>()); }
                    compact
                }).collect();
            let request = json!({"command":"review","video":output.file,"sidecar":sidecar,
                "steamId":player_id,"model":review["model"],"expectedModelDigest":review["modelDigest"],"threshold":review["threshold"],"reviewMode":review["mode"],
                "profileHash":review["profileHash"],"learningProfile":review["learningProfile"],
                "discoveryCandidates":selected_candidates,"sourceDigest":review["demoDigest"]});
            if let Err(error) = self.start(request) { self.review_error(&error); }
        } else {
            let mut complete = self.review.lock().unwrap();
            let mut coverage = complete["coverage"].as_array().cloned().unwrap_or_default();
            if let Some(snapshot) = &job.source_snapshot {
                for highlight in &snapshot.highlights {
                    if !outputs.iter().any(|o| o.highlight_id.as_deref() == Some(highlight.id.as_str())) {
                        coverage.push(json!({"startTick":highlight.start_tick,"endTick":highlight.end_tick,"round":highlight.round,
                            "level":"unavailable","status":"preview-unavailable","reason":job.error.as_deref().unwrap_or("This preview could not be recorded.")}));
                    }
                }
            }
            complete["coverage"] = json!(coverage);
            complete["unavailableWindows"] = json!(complete["coverage"].as_array().map_or(0, |items| items.iter().filter(|r| r["level"] == "unavailable").count()));
            let complete_coverage = outputs.len() == job.highlight_ids.len() && complete["reports"].as_array().is_some_and(|reports| reports.iter().all(|r| r["completeCoverage"] == true));
            complete["completeCoverage"] = json!(complete_coverage);
            complete["wholeMatchCoverage"] = json!(complete_coverage && review["mode"] == "full-sweep");
            complete["status"] = json!(if job.status == JobStatus::Partial {
                "Review finished; some preview batches could not be recorded"
            } else {
                if review["mode"] == "full-sweep" { "Full visual sweep finished" } else { "Targeted review finished" }
            });
            complete["complete"] = json!(true);
            complete["phase"] = json!("complete");
            complete["completed"] = json!(outputs.len());
            complete["total"] = json!(outputs.len());
            self.save_review(&complete);
            let moments = complete["moments"].as_array().cloned().unwrap_or_default();
            let auto_export = complete["autoExport"] == true;
            let render_defaults: RenderOptions = serde_json::from_value(complete["renderDefaults"].clone()).unwrap_or_else(|_| self.engine.recording_defaults());
            drop(complete);
            if auto_export {
                for moment in moments {
                    if let (Some(start), Some(end), Some(title)) = (
                        moment["startTick"].as_i64(),
                        moment["endTick"].as_i64(),
                        moment["title"].as_str(),
                    ) {
                        if start >= 0 && end <= i32::MAX as i64 && end > start {
                            let _ = self.engine.enqueue_automatic_custom_render(
                                demo_id,
                                player_id,
                                vec![[start as i32, end as i32]],
                                title.to_string(),
                                render_defaults.clone(),
                            );
                        }
                    }
                }
            }
        }
    }

    fn accept_review(&self, result: &Value) {
        let mut review = self.review.lock().unwrap();
        if !review.is_object() || review["complete"] != false || review["cancelling"] == true {
            return;
        }
        if !frozen_review_result_matches(&review, result) {
            review["status"] = json!("Needs attention");
            review["error"] = json!("The AI result does not match this review's frozen source, player, model or preferences. Start a new review.");
            review["phase"] = json!("error");
            review["complete"] = json!(true);
            self.save_review(&review);
            return;
        }
        let mut moments = review["moments"].as_array().cloned().unwrap_or_default();
        if let Some(found) = result["moments"].as_array() {
            for moment in found {
                if !moments.iter().any(|m| {
                    m["startTick"] == moment["startTick"] && m["endTick"] == moment["endTick"] && m["steamId"] == moment["steamId"]
                }) {
                    moments.push(moment.clone());
                }
            }
        }
        review["moments"] = json!(moments);
        let mut reports = review["reports"].as_array().cloned().unwrap_or_default();
        reports.push(json!({"reportPath":result["reportPath"],"sourceSHA256":result["sourceSHA256"],
            "reviewedWindows":result["reviewedWindows"],"totalWindows":result["totalWindows"],"completeCoverage":result["completeCoverage"]}));
        review["reports"] = json!(reports);
        let mut coverage = review["coverage"].as_array().cloned().unwrap_or_default();
        coverage.extend(result["coverage"].as_array().into_iter().flatten().cloned());
        review["coverage"] = json!(coverage);
        review["reviewedWindows"] = json!(review["reviewedWindows"].as_u64().unwrap_or(0) + result["reviewedWindows"].as_u64().unwrap_or(0));
        review["unavailableWindows"] = json!(review["unavailableWindows"].as_u64().unwrap_or(0) + result["coverage"].as_array().map_or(0, |items| items.iter().filter(|r| r["level"] == "unavailable").count()) as u64);
        review["nextOutput"] = json!(review["nextOutput"].as_u64().unwrap_or(0) + 1);
        review["completed"] = review["nextOutput"].clone();
        review["status"] = json!("Reviewing the match");
        self.save_review(&review);
    }

    fn queue_own_moments(&self, settings: &Value) {
        if settings["autoExport"].as_bool() != Some(true) {
            return;
        }
        let Some(player) = settings["steamId"].as_str().filter(|v| !v.is_empty()) else {
            return;
        };
        let min_kills = settings["minKills"].as_u64().unwrap_or(2) as usize;
        let deaths = settings["saveDeaths"].as_bool().unwrap_or(true);
        for meta in self.engine.list_demos() {
            let Some(parsed) = self.engine.parsed(&meta.id) else {
                continue;
            };
            let ids = parsed
                .highlights
                .iter()
                .filter(|h| {
                    if h.player.steamid != player {
                        return false;
                    }
                    if h.tags.iter().any(|t| t == "death") {
                        return deaths;
                    }
                    h.kills.len() >= min_kills
                        || h.tags.iter().any(|t| {
                            [
                                "ace",
                                "clutch",
                                "noscope",
                                "wallbang",
                                "knife",
                                "airborne",
                                "thrusmoke",
                                "blind",
                                "zeus",
                                "posthumous",
                                "ninja-defuse",
                                "exceptional-shot",
                                "long-range-headshot",
                            ]
                            .contains(&t.as_str())
                        })
                })
                .map(|h| h.id.clone())
                .collect();
            let _ = self.engine.enqueue_automatic_render(
                &meta.id,
                ids,
                self.engine.recording_defaults(),
            );
        }
    }

    fn call(
        &self,
        mut request: Value,
        cancel: Arc<AtomicBool>,
        report_progress: bool,
    ) -> Result<Value, String> {
        if matches!(request["command"].as_str(), Some("sync" | "retry")) {
            if let Some(cs2) = self.engine.tool_paths().cs2_exe {
                request["_installedCs2Exe"] = json!(cs2);
            }
        }
        let source = serde_json::to_vec(&request).map_err(|_| "Invalid automation request")?;
        if source.len() > 1024 * 1024 {
            return Err("The automation request is too large.".into());
        }
        let tree = ProcessTree::new().map_err(|_| "Cannot create the automation process group.")?;
        let local_python = workspace::root().join("tools/python-env/Scripts/python.exe");
        let python = if local_python.is_file() { local_python } else { workspace::root().join("tools/python/python.exe") };
        let mut command = Command::new(python);
        command
            .arg("-u")
            .arg(workspace::root().join("app/automation/cli.py"))
            .current_dir(workspace::root().join("app/automation"))
            .env("PYTHONNOUSERSITE", "1")
            .env("PYTHONPATH", "")
            .env_remove("PYTHONHOME")
            .env(
                "PYTHONPYCACHEPREFIX",
                workspace::root().join("cache/pycache"),
            )
            .env("HF_HOME", workspace::root().join("models/huggingface"))
            .env("XDG_CACHE_HOME", workspace::root().join("cache/python"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command
            .spawn()
            .map_err(|_| "The local Python helper could not start.")?;
        // The helper blocks on stdin until assignment succeeds. It cannot spawn descendants yet.
        if tree.attach_child(&child).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Cannot isolate the local automation helper.".into());
        }
        let mut input = child
            .stdin
            .take()
            .ok_or("Automation input is unavailable")?;
        input
            .write_all(&source)
            .map_err(|_| "Cannot send the automation request")?;
        drop(input);
        let stdout = child
            .stdout
            .take()
            .ok_or("Automation output is unavailable")?;
        let output = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
            Ok::<_, std::io::Error>(bytes)
        });
        let stderr = child
            .stderr
            .take()
            .ok_or("Automation progress is unavailable")?;
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                loop {
                    line.clear();
                    if Read::by_ref(&mut reader)
                        .take(8193)
                        .read_line(&mut line)
                        .unwrap_or(0)
                        == 0
                    {
                        break;
                    }
                    if line.len() > 8192 {
                        break;
                    }
                    if report_progress {
                        if let Ok(data) = serde_json::from_str::<Value>(&line) {
                            if let Some(message) =
                                data["progress"].as_str().filter(|v| v.len() <= 2048)
                            {
                                let mut activity = self.activity.lock().unwrap();
                                if activity["cancelling"] != true {
                                    activity["progress"] = json!(message);
                                    if let Some(phase) = data["phase"].as_str().filter(|p| {
                                        p.len() <= 64
                                            && p.bytes()
                                                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                                    }) {
                                        if activity["phase"].as_str() != Some(phase) {
                                            activity["completed"] = Value::Null;
                                            activity["total"] = Value::Null;
                                        }
                                        activity["phase"] = json!(phase);
                                    }
                                    for field in ["completed", "total"] {
                                        if let Some(count) = data[field].as_u64() {
                                            activity[field] = json!(count);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            });
            let deadline =
                Instant::now() + Duration::from_secs(if report_progress { 12 * 3600 } else { 30 });
            loop {
                if cancel.load(Ordering::SeqCst) || self.stopped.load(Ordering::SeqCst) {
                    let _ = tree.finish();
                    let _ = child.wait();
                    return Err("The task was cancelled.".into());
                }
                if Instant::now() >= deadline {
                    let _ = tree.finish();
                    let _ = child.wait();
                    return Err("The local task timed out.".into());
                }
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let _ = tree.finish();
                        if !status.success() {
                            return Err("The local automation helper stopped unexpectedly.".into());
                        }
                        break;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                    Err(_) => {
                        let _ = tree.finish();
                        return Err("Cannot read automation task status.".into());
                    }
                }
            }
            let bytes = output
                .join()
                .map_err(|_| "Automation output failed")?
                .map_err(|_| "Cannot read automation results")?;
            if bytes.len() > 4 * 1024 * 1024 {
                return Err("The automation result exceeds its safety limit.".into());
            }
            let response: Value = serde_json::from_slice(&bytes)
                .map_err(|_| "The automation helper returned invalid data")?;
            if response["ok"].as_bool() != Some(true) {
                if response["deferred"] == true {
                    return Err(format!(
                        "{DEFERRED}{}",
                        response["error"]
                            .as_str()
                            .unwrap_or("Waiting for CS2 to close")
                    ));
                }
                return Err(response["error"]
                    .as_str()
                    .unwrap_or("The local task failed")
                    .to_string());
            }
            Ok(response["result"].clone())
        })
    }
}

fn finalize_activity(mut activity: Value, previous: &Value, cancelled: bool) -> Value {
    for field in ["task", "demoId", "startedAt", "completed", "total"] {
        activity[field] = previous[field].clone();
    }
    let phase = activity["phase"].as_str().unwrap_or("complete").to_string();
    activity["phase"] = json!(if cancelled {
        "cancelled"
    } else if activity["error"].is_string() {
        "failed"
    } else {
        phase.as_str()
    });
    activity["cancelling"] = json!(false);
    activity
}

fn frozen_review_result_matches(review: &Value, result: &Value) -> bool {
    result["sourceDigest"] == review["demoDigest"] && result["steamId"] == review["playerId"]
        && result["model"] == review["model"] && (review["modelDigest"].is_null() || result["modelDigest"] == review["modelDigest"])
        && result["profileHash"] == review["profileHash"] && result["reviewMode"] == review["mode"]
}

fn cancellation_cleaned_up(review: &Value, helper_busy: bool, active_job: Option<&str>) -> bool {
    review["complete"] == false
        && review["cancelling"] == true
        && !helper_busy
        && !review["jobId"]
            .as_str()
            .is_some_and(|id| active_job == Some(id))
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[test]
    fn review_results_match_frozen_source_player_model_digest_and_preferences() {
        let review=json!({"demoDigest":"source-a","playerId":"player-a","model":"model-a","modelDigest":"weights-a","profileHash":"profile-a","mode":"targeted"});
        let result=json!({"sourceDigest":"source-a","steamId":"player-a","model":"model-a","modelDigest":"weights-a","profileHash":"profile-a","reviewMode":"targeted"});
        assert!(frozen_review_result_matches(&review,&result));
        for key in ["sourceDigest","steamId","model","modelDigest","profileHash","reviewMode"] {
            let mut changed=result.clone();changed[key]=json!("different");assert!(!frozen_review_result_matches(&review,&changed),"{key}");
            changed[key]=Value::Null;assert!(!frozen_review_result_matches(&review,&changed),"missing {key}");
        }
    }
    #[test]
    fn review_files_remain_reserved_during_helper_cleanup_and_between_rounds() {
        struct Silent;
        impl demodesk_core::engine::Notify for Silent {
            fn notify(&self, _: demodesk_core::engine::Event) {}
        }
        let temporary = workspace::root().join("temp");
        std::fs::create_dir_all(&temporary).unwrap();
        let root = temporary.join(format!(
            "review-files-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let store = demodesk_core::store::Store::open(root.clone()).unwrap();
        store
            .save_settings(&demodesk_core::store::Settings {
                scan_game_replays: false,
                ..Default::default()
            })
            .unwrap();
        let mut job = store
            .new_job("preview-fixture", vec![], Default::default())
            .unwrap();
        job.status = demodesk_core::store::JobStatus::Done;
        store.save_job(&job).unwrap();
        let engine = Engine::new(root.clone(), Arc::new(Silent)).unwrap();
        let reserved = engine.reserve_job(&job.id).unwrap();
        // Simulate only lifecycle bookkeeping; no Python, AI or game process starts.
        let state = Automation {
            engine: engine.clone(),
            cached: Mutex::new(Value::Null),
            activity: Mutex::new(Value::Null),
            current: Mutex::new(Some(Arc::new(AtomicBool::new(false)))),
            dispatch: Mutex::new(()),
            review: Mutex::new(json!({"jobId":job.id,"complete":true})),
            review_files: Mutex::new(Some(reserved)),
            stopped: AtomicBool::new(false),
        };
        state.release_review_files();
        assert!(engine.delete_job(&job.id).is_err());
        *state.current.lock().unwrap() = None;
        state.review.lock().unwrap()["complete"] = json!(false);
        state.release_review_files();
        assert!(engine.delete_job(&job.id).is_err());
        state.review.lock().unwrap()["complete"] = json!(true);
        state.release_review_files();
        engine.delete_job(&job.id).unwrap();
        assert!(state.review_files.lock().unwrap().is_none());
        drop(state);
        drop(engine);
        drop(store);
        assert!(root
            .canonicalize()
            .unwrap()
            .starts_with(temporary.canonicalize().unwrap()));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn deferred_activity_retains_its_waiting_reason_and_task_for_manual_retry() {
        let previous = json!({"busy":true,"task":"sync","demoId":"match","startedAt":"2026-10-13T01:00:00Z","completed":2,"total":9,"cancelling":false});
        for phase in ["waiting-for-game", "waiting-for-compute"] {
            let finished = finalize_activity(
                json!({"busy":false,"phase":phase,"progress":"Processing waits until the resource is available."}),
                &previous,
                false,
            );
            assert_eq!(finished["phase"], phase);
            assert_eq!(finished["busy"], false);
            assert_eq!(finished["task"], "sync");
            assert_eq!(finished["demoId"], "match");
            assert_eq!(finished["completed"], 2);
            assert_eq!(finished["total"], 9);
            assert_eq!(finished["startedAt"], previous["startedAt"]);
            assert_eq!(finished["cancelling"], false);
            assert!(finished["progress"].as_str().unwrap().contains("waits"));
        }
    }
    #[test]
    fn terminal_activity_overrides_waiting_only_for_failure_or_cancellation() {
        let waiting = json!({"busy":false,"phase":"waiting-for-game","progress":"Waiting for CS2"});
        assert_eq!(
            finalize_activity(waiting.clone(), &Value::Null, true)["phase"],
            "cancelled"
        );
        let failed = json!({"busy":false,"phase":"waiting-for-compute","error":"Helper stopped"});
        assert_eq!(
            finalize_activity(failed.clone(), &Value::Null, false)["phase"],
            "failed"
        );
        assert_eq!(
            finalize_activity(failed, &Value::Null, true)["phase"],
            "cancelled"
        );
        assert_eq!(
            finalize_activity(
                json!({"busy":false,"progress":"Finished"}),
                &Value::Null,
                false
            )["phase"],
            "complete"
        );
    }
    #[test]
    fn cancellation_finishes_only_after_its_owned_preview_and_helper_stop() {
        let review = json!({"complete":false,"cancelling":true,"jobId":"preview"});
        assert!(!cancellation_cleaned_up(&review, true, None));
        assert!(!cancellation_cleaned_up(&review, false, Some("preview")));
        assert!(cancellation_cleaned_up(&review, false, None));
        assert!(cancellation_cleaned_up(
            &review,
            false,
            Some("unrelated-export")
        ));
        assert!(!cancellation_cleaned_up(
            &json!({"complete":true,"cancelling":true}),
            false,
            None
        ));
    }
}
