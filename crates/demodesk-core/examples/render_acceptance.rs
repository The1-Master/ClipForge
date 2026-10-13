//! Real personal-demo acceptance through the production Engine render queue.
//! Preparation never launches CS2. --run requires the user-authorized idle test.
use anyhow::{anyhow, ensure, Result};
use demodesk_core::{
    engine::{Engine, Event, Notify},
    render::{cs2_is_running, RenderOptions},
    store::{DemoStatus, JobStatus, Settings},
};
use serde_json::json;
use serde::Deserialize;
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const ROOT: &str = "J:/CS2ClipStudio";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    demo_path: PathBuf,
    player_id: String,
    start_tick: i32,
    end_tick: i32,
    event_tick: i32,
    headshot: bool,
    weapon: String,
}

struct Events(Mutex<File>);
impl Notify for Events {
    fn notify(&self, event: Event) {
        let row = match event {
            Event::JobChanged { job } => json!({"type":"job", "id":job.id, "status":job.status,
                "stage":job.stage, "progress":job.progress, "error":job.error, "lastLog":job.log.last()}),
            Event::DemoChanged { demo } => json!({"type":"demo", "id":demo.id,
                "status":demo.status, "error":demo.error}),
            _ => return,
        };
        if let Ok(mut file) = self.0.lock() {
            let _ = writeln!(file, "{row}");
            let _ = file.flush();
        }
    }
}

fn benchmark_root(path: &Path) -> Result<PathBuf> {
    let allowed = Path::new(ROOT).join("research/benchmarks/render").canonicalize()?;
    // Validate the parent before creating this one benchmark directory.
    let parent = path.parent().ok_or_else(|| anyhow!("benchmark folder has no parent"))?.canonicalize()?;
    ensure!(parent.starts_with(&allowed), "benchmark must stay in the J: render research folder");
    fs::create_dir_all(path)?;
    let actual = path.canonicalize()?;
    ensure!(actual.starts_with(&allowed), "resolved benchmark folder leaves the project");
    // Keep ordinary drive paths for CS2/HLAE; canonicalization above is for the
    // ownership check, not their command-line spelling (\\?\ is Rust/Win32).
    Ok(std::path::absolute(path)?)
}

fn main() -> Result<()> {
    let arguments: Vec<String> = std::env::args().collect();
    ensure!(arguments.len() == 3 && ["--prepare", "--run"].contains(&arguments[2].as_str()),
        "usage: render_acceptance <J: render benchmark folder> --prepare|--run");
    let destination = benchmark_root(Path::new(&arguments[1]))?;
    let run = arguments[2] == "--run";
    let data = destination.join("state");
    let engine = Engine::new(data, Arc::new(Events(Mutex::new(File::create(destination.join("events.jsonl"))?))))?;
    engine.set_queue_paused(true)?;
    engine.save_settings(Settings {
        steam_dir: Some("C:/Program Files (x86)/Steam".into()),
        cs2_dir: Some("F:/SteamLibrary/steamapps/common/Counter-Strike Global Offensive".into()),
        hlae_exe: Some(format!("{ROOT}/tools/hlae/HLAE.exe")),
        ffmpeg_exe: Some(format!("{ROOT}/tools/ffmpeg/ffmpeg-master-latest-win64-gpl/bin/ffmpeg.exe")),
        scan_game_replays: false,
        ..Settings::default()
    }).map_err(|issues| anyhow!(issues.join("; ")))?;
    let doctor = engine.doctor();
    ensure!(doctor.ok, "renderer prerequisites failed: {:?}", doctor.problems);
    let request_path = Path::new(ROOT).join("work/native-personal-acceptance.json");
    ensure!(fs::metadata(&request_path)?.len() <= 65536, "acceptance request is too large");
    let request: Request = serde_json::from_slice(&fs::read(&request_path)?)?;
    let demo_path = request.demo_path.canonicalize()?;
    ensure!(demo_path.starts_with(Path::new(ROOT).canonicalize()?), "demo must stay in the J: project");
    ensure!(request.start_tick < request.event_tick && request.event_tick < request.end_tick,
        "acceptance window does not cover its requested event");
    ensure!(request.demo_path.is_absolute(), "acceptance demo path must be absolute");
    let meta = engine.add_demo(&request.demo_path)?;
    engine.parse_demo(&meta.id)?;
    let parse_deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let (current, parsed) = engine.get_demo(&meta.id).ok_or_else(|| anyhow!("demo disappeared"))?;
        if parsed.is_some() && current.status == DemoStatus::Parsed { break; }
        ensure!(current.status != DemoStatus::Error, "reference parse failed: {:?}", current.error);
        ensure!(Instant::now() < parse_deadline, "reference parse timed out");
        thread::sleep(Duration::from_millis(100));
    }
    let options = RenderOptions { keep_raw_files: true, ..RenderOptions::default() };
    let parsed = engine.parsed(&meta.id).ok_or_else(|| anyhow!("parsed demo disappeared"))?;
    ensure!(parsed.info.players.iter().any(|p| p.steamid == request.player_id), "configured player is not in this demo");
    let configuration = json!({"fixture":demo_path.file_name(), "playerSource":"runtime request",
        "demoTicks":[request.start_tick,request.end_tick], "eventTick":request.event_tick,
        "weapon":request.weapon, "headshot":request.headshot,
        "expectedDurationSeconds":(request.end_tick-request.start_tick) as f64 / parsed.info.tick_rate,
        "options":options, "doctor":doctor,
        "cancelMarker":destination.join("cancel")});
    fs::write(destination.join("request.json"), serde_json::to_vec_pretty(&configuration)?)?;
    println!("{}", json!({"stage":"prepared", "destination":destination, "gameLaunchRequested":run}));
    if !run { engine.shutdown(); return Ok(()); }
    ensure!(!destination.join("cancel").exists(), "remove stale cancellation marker before a new authorized run");
    ensure!(!cs2_is_running()?, "CS2 is open; recording is deferred");
    let job = engine.enqueue_custom_render(&meta.id, &request.player_id, vec![[request.start_tick,request.end_tick]],
        "Personal gameplay: native 4K60 acceptance".into(), options)?;
    fs::write(destination.join("job-id.txt"), &job.id)?;
    engine.set_queue_paused(false)?;
    let deadline = Instant::now() + Duration::from_secs(600);
    let mut last = String::new();
    loop {
        let latest = engine.list_jobs()?.into_iter().find(|entry| entry.id == job.id)
            .ok_or_else(|| anyhow!("render job disappeared"))?;
        let message = json!({"id":latest.id, "status":latest.status, "stage":latest.stage,
                            "progress":latest.progress, "lastLog":latest.log.last()}).to_string();
        if message != last { println!("{message}"); last = message; }
        if matches!(latest.status, JobStatus::Done | JobStatus::Partial | JobStatus::Error | JobStatus::Cancelled) {
            fs::write(destination.join("result.json"), serde_json::to_vec_pretty(&latest)?)?;
            engine.shutdown();
            println!("{}", json!({"stage":"finished", "status":latest.status, "outputs":latest.outputs,
                                  "error":latest.error, "destination":destination}));
            ensure!(latest.status == JobStatus::Done, "native render acceptance failed: {:?}", latest.error);
            return Ok(());
        }
        if destination.join("cancel").exists() || Instant::now() > deadline {
            engine.cancel_job(&job.id);
        }
        thread::sleep(Duration::from_millis(200));
    }
}
