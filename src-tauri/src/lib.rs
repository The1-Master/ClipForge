//! CS DemoDesk — Tauri glue: exposes the engine as commands and forwards engine events to the
//! window. All real logic lives in `demodesk-core`.

use demodesk_core::engine::{Detected, Engine, Event, Notify, SetupState};
use demodesk_core::radar::MapAssets;
use demodesk_core::render::{DoctorReport, RenderOptions, SetupTool};
use demodesk_core::store::{DemoMeta, RenderJob, Settings};
use demodesk_core::KillEvent;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

mod automation;
mod community;
mod data_directory;
#[cfg(windows)]
mod webview_runtime;
mod workspace;
use data_directory::DataDirectory;
type Directory = Arc<DataDirectory>;

pub const EVENT_NAME: &str = "demodesk://event";

struct TauriNotify(AppHandle);
impl Notify for TauriNotify {
    fn notify(&self, event: Event) {
        let _ = self.0.emit(EVENT_NAME, event);
    }
}

type Eng = Arc<Engine>;
type Auto = Arc<automation::Automation>;
type Community = Arc<community::Community>;
type CmdResult<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    format!("{e:#}")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    ok: bool,
    missing_render_tools: Vec<&'static str>,
    problems: Vec<String>,
    data_dir: PathBuf,
    active_render: Option<String>,
    version: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsResponse {
    settings: Settings,
    detected: Detected,
    doctor: DoctorReport,
    setup: std::collections::HashMap<SetupTool, SetupState>,
    tool_checks:
        std::collections::HashMap<SetupTool, demodesk_core::render::diagnostics::ToolCheck>,
    data_dir: PathBuf,
    data_dir_override: Option<PathBuf>,
    default_data_dir: PathBuf,
    restart_required: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageBytes {
    /// size of the stored parse results (demodesk-data/parsed)
    parsed_bytes: u64,
    anomaly_bytes: u64,
    /// size of the rendered videos (demodesk-data/clips)
    clips_bytes: u64,
    /// size of the extracted radar images (demodesk-data/radar)
    radar_bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DemoResponse {
    meta: DemoMeta,
    /// ParsedDemo without the (large) kills array
    parsed: Option<serde_json::Value>,
}

fn settings_response(engine: &Engine, directory: &DataDirectory) -> SettingsResponse {
    let selected = directory.selected();
    let restart_required = selected.as_ref().unwrap_or(&directory.default) != &directory.active;
    SettingsResponse {
        data_dir_override: selected,
        default_data_dir: directory.default.clone(),
        restart_required,
        settings: engine.settings(),
        detected: engine.detected(),
        doctor: engine.doctor(),
        setup: engine.setup_state(),
        tool_checks: engine.tool_checks(),
        data_dir: engine.data_dir().to_path_buf(),
    }
}

/// Every engine call does file I/O (settings, parse results, job records) or
/// more; run it on the blocking pool so the async runtime stays responsive.
async fn blocking<T: Send + 'static>(
    engine: &Eng,
    f: impl FnOnce(&Eng) -> CmdResult<T> + Send + 'static,
) -> CmdResult<T> {
    let engine = engine.clone();
    tauri::async_runtime::spawn_blocking(move || f(&engine))
        .await
        .map_err(err)?
}

#[tauri::command]
async fn check_for_updates(
    window: tauri::WebviewWindow,
) -> CmdResult<demodesk_core::updates::Update> {
    // This local fork has no published updater. Never replace it with an upstream binary.
    let _ = window;
    Ok(demodesk_core::updates::Update::Current)
}
fn existing_browse_directory(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    if path.is_dir() {
        return Some(path.to_path_buf());
    }
    path.parent()
        .filter(|parent| parent.is_dir())
        .map(Path::to_path_buf)
}

#[tauri::command]
fn browse_directory(app: AppHandle, path: Option<String>) -> CmdResult<PathBuf> {
    if let Some(directory) = path
        .as_deref()
        .and_then(|value| existing_browse_directory(Path::new(value.trim())))
    {
        return Ok(directory);
    }
    app.path().desktop_dir().map_err(err)
}

#[cfg(test)]
mod browse_tests {
    #[test]
    fn existing_file_directory_and_missing_path() {
        let exe = std::env::current_exe().unwrap();
        let parent = exe.parent().unwrap();
        assert_eq!(
            super::existing_browse_directory(&exe),
            Some(parent.to_path_buf())
        );
        assert_eq!(
            super::existing_browse_directory(parent),
            Some(parent.to_path_buf())
        );
        assert_eq!(
            super::existing_browse_directory(
                &parent.join("missing-browse-test-dir").join("tool.exe")
            ),
            None
        );
        assert_eq!(
            super::existing_browse_directory(std::path::Path::new("relative.exe")),
            None
        );
    }
}

#[tauri::command]
async fn get_status(engine: State<'_, Eng>) -> CmdResult<Status> {
    blocking(&engine, |e| {
        let d = e.doctor();
        let mut missing_render_tools = Vec::new();
        if d.paths.steam_dir.is_none() {
            missing_render_tools.push("Steam");
        }
        if d.paths.cs2_exe.is_none() {
            missing_render_tools.push("CS2");
        }
        if d.paths.hlae_exe.is_none() || d.paths.hlae_dll.is_none() {
            missing_render_tools.push("HLAE");
        }
        if d.paths.ffmpeg_exe.is_none() {
            missing_render_tools.push("ffmpeg");
        }
        Ok(Status {
            missing_render_tools,
            ok: d.ok,
            problems: d.problems,
            data_dir: e.data_dir().to_path_buf(),
            active_render: e.active_job_id(),
            version: env!("CARGO_PKG_VERSION").into(),
        })
    })
    .await
}

#[tauri::command]
async fn get_storage_bytes(engine: State<'_, Eng>) -> CmdResult<StorageBytes> {
    blocking(&engine, |e| {
        let (parsed_bytes, clips_bytes, radar_bytes, anomaly_bytes) = e.storage_bytes();
        Ok(StorageBytes {
            parsed_bytes,
            clips_bytes,
            radar_bytes,
            anomaly_bytes,
        })
    })
    .await
}

#[tauri::command]
async fn get_settings(
    engine: State<'_, Eng>,
    directory: State<'_, Directory>,
) -> CmdResult<SettingsResponse> {
    let directory = directory.inner().clone();
    blocking(&engine, move |e| Ok(settings_response(e, &directory))).await
}

fn validate_settings(engine: &Engine, settings: &Settings) -> Vec<String> {
    let mut problems = engine.validate_settings(settings);
    match DataDirectory::settings_use_app_data(settings) {
        Ok(true) => problems.push("Choose settings directories outside AppData.".into()),
        Err(error) => problems.push(error),
        Ok(false) => {}
    }
    for path in [&settings.hlae_exe, &settings.ffmpeg_exe, &settings.vrf_exe]
        .into_iter()
        .flatten()
    {
        let directory = demodesk_core::render::paths::installation_directory(Path::new(path));
        if let Err(error) = DataDirectory::validate_tool_directory(&directory) {
            problems.push(error);
        }
    }
    problems
}

#[tauri::command]
async fn preview_settings(
    engine: State<'_, Eng>,
    directory: State<'_, Directory>,
    settings: Settings,
    original_settings: Settings,
    data_dir_override: Option<String>,
) -> CmdResult<serde_json::Value> {
    let directory = directory.inner().clone();
    blocking(&engine, move |e| {
        let target = directory
            .validate(data_dir_override)?
            .ok_or("Missing data directory")?;
        let merged = directory.target_settings(&target, &original_settings, &settings)?;
        let problems = validate_settings(e, &merged);
        if !problems.is_empty() {
            return Err(problems.join("\n"));
        }
        Ok(serde_json::json!({ "target": target, "restartRequired": directory.changes(&target) }))
    })
    .await
}

#[tauri::command]
async fn save_settings(
    engine: State<'_, Eng>,
    directory: State<'_, Directory>,
    settings: Settings,
    original_settings: Settings,
    data_dir_override: Option<String>,
    change_confirmed: bool,
) -> CmdResult<SettingsResponse> {
    let directory = directory.inner().clone();
    blocking(&engine, move |e| {
        let target = directory
            .validate(data_dir_override)?
            .ok_or("Missing data directory")?;
        let merged = directory.target_settings(&target, &original_settings, &settings)?;
        let problems = validate_settings(e, &merged);
        if !problems.is_empty() {
            return Err(problems.join("\n"));
        }
        if directory.changes(&target) {
            if !change_confirmed {
                return Err("Confirm the data directory change before saving.".into());
            }
            directory.commit_switch(target, &merged)?;
        } else {
            e.save_settings(merged)
                .map_err(|problems| problems.join("\n"))?;
            directory.save(Some(target))?;
        }
        Ok(settings_response(e, &directory))
    })
    .await
}

#[tauri::command]
async fn run_setup(
    engine: State<'_, Eng>,
    tool: SetupTool,
    force: bool,
    directory: Option<String>,
) -> CmdResult<bool> {
    blocking(&engine, move |e| {
        let directory = directory
            .filter(|value| !value.trim().is_empty())
            .map(|value| DataDirectory::validate_tool_directory(Path::new(value.trim())))
            .transpose()?;
        Ok(e.start_setup(tool, force, directory))
    })
    .await
}

#[tauri::command]
fn cancel_setup(engine: State<'_, Eng>, tool: SetupTool) {
    engine.cancel_setup(tool);
}

#[tauri::command]
async fn check_tools(
    engine: State<'_, Eng>,
    directory: State<'_, Directory>,
) -> CmdResult<SettingsResponse> {
    let directory = directory.inner().clone();
    blocking(&engine, move |e| {
        e.check_tools();
        Ok(settings_response(e, &directory))
    })
    .await
}

#[tauri::command]
async fn check_tool_update(
    engine: State<'_, Eng>,
    tool: SetupTool,
) -> CmdResult<demodesk_core::render::setup::ToolUpdate> {
    blocking(&engine, move |e| e.check_tool_update(tool).map_err(err)).await
}

#[tauri::command]
async fn tool_diagnostics(
    engine: State<'_, Eng>,
    directory: State<'_, Directory>,
) -> CmdResult<String> {
    let directory = directory.inner().clone();
    blocking(&engine, move |e| {
        let mut report = e.tool_diagnostics();
        let selected = directory
            .selected()
            .unwrap_or_else(|| directory.default.clone());
        if directory.changes(&selected) {
            report["pendingDataDirectory"] = serde_json::json!(selected);
        }
        serde_json::to_string_pretty(&report).map_err(err)
    })
    .await
}

#[tauri::command]
async fn list_demos(engine: State<'_, Eng>) -> CmdResult<Vec<DemoMeta>> {
    blocking(&engine, |e| Ok(e.list_demos())).await
}

#[tauri::command]
async fn register_demo(engine: State<'_, Eng>, path: String) -> CmdResult<DemoMeta> {
    blocking(&engine, move |e| e.add_demo(Path::new(&path)).map_err(err)).await
}

#[tauri::command]
async fn parse_demo(engine: State<'_, Eng>, id: String) -> CmdResult<()> {
    blocking(&engine, move |e| e.parse_demo(&id).map_err(err)).await
}

#[tauri::command]
async fn get_demo(engine: State<'_, Eng>, id: String) -> CmdResult<DemoResponse> {
    blocking(&engine, move |e| {
        let (meta, parsed) = e.get_demo(&id).ok_or("demo not found")?;
        Ok(DemoResponse {
            meta,
            parsed: parsed.map(|p| p.without_kills()),
        })
    })
    .await
}

#[tauri::command]
async fn scoring_history(
    engine: State<'_, Eng>,
    id: String,
) -> CmdResult<std::collections::BTreeMap<String, Vec<demodesk_core::scoring::Assessment>>> {
    blocking(&engine, move |e| e.scoring_match_history(&id).map_err(err)).await
}

#[tauri::command]
async fn score_match(
    engine: State<'_, Eng>,
    id: String,
    force: bool,
) -> CmdResult<demodesk_core::scoring::queue::Job> {
    blocking(&engine, move |e| {
        e.enqueue_analysis(&id, force).map_err(err)
    })
    .await
}

#[tauri::command]
async fn analysis_jobs(
    engine: State<'_, Eng>,
) -> CmdResult<Vec<demodesk_core::scoring::queue::Job>> {
    blocking(&engine, |e| Ok(e.analysis_jobs())).await
}

#[tauri::command]
async fn get_kills(engine: State<'_, Eng>, id: String) -> CmdResult<Vec<KillEvent>> {
    blocking(&engine, move |e| {
        e.parsed(&id)
            .map(|p| p.kills.clone())
            .ok_or_else(|| "demo not parsed".into())
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayFile {
    path: PathBuf,
    bytes: u64,
}

/// Position stream for the 2D replay; built on first call (a few seconds).
#[tauri::command]
async fn get_replay(engine: State<'_, Eng>, id: String) -> CmdResult<ReplayFile> {
    blocking(&engine, move |e| {
        let path = e.replay_file(&id).map_err(err)?;
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Ok(ReplayFile { path, bytes })
    })
    .await
}

/// Radar image(s) for a map, extracted from the game files on first call.
#[tauri::command]
async fn get_map_assets(engine: State<'_, Eng>, map_name: String) -> CmdResult<MapAssets> {
    blocking(&engine, move |e| e.map_assets(&map_name).map_err(err)).await
}

#[tauri::command]
async fn clear_radar(engine: State<'_, Eng>) -> CmdResult<u64> {
    blocking(&engine, |e| Ok(e.clear_radar())).await
}

#[tauri::command]
async fn clear_analysis(engine: State<'_, Eng>, id: String) -> CmdResult<()> {
    blocking(&engine, move |e| e.clear_analysis(&id).map_err(err)).await
}

#[tauri::command]
async fn clear_match_anomaly(engine: State<'_, Eng>, id: String) -> CmdResult<()> {
    blocking(&engine, move |e| e.clear_match_anomaly(&id).map_err(err)).await
}

#[tauri::command]
async fn clear_anomaly_data(engine: State<'_, Eng>) -> CmdResult<u64> {
    blocking(&engine, |e| e.clear_anomaly_data().map_err(err)).await
}

#[tauri::command]
async fn clear_all_analysis(engine: State<'_, Eng>) -> CmdResult<u64> {
    blocking(&engine, |e| e.clear_all_analysis().map_err(err)).await
}

#[tauri::command]
async fn remove_demo(engine: State<'_, Eng>, id: String) -> CmdResult<()> {
    blocking(&engine, move |e| e.remove_demo(&id).map_err(err)).await
}

#[tauri::command]
async fn analysis_clips(
    engine: State<'_, Eng>,
    demo_id: String,
    selection: demodesk_core::scoring::clips::Selection,
    merge: Option<bool>,
) -> CmdResult<Vec<demodesk_core::scoring::clips::RuleClips>> {
    blocking(&engine, move |e| {
        e.analysis_clips(&demo_id, &selection, merge.unwrap_or(false))
            .map_err(err)
    })
    .await
}
#[tauri::command]
async fn start_analysis_render(
    engine: State<'_, Eng>,
    demo_id: String,
    selection: demodesk_core::scoring::clips::Selection,
    options: RenderOptions,
) -> CmdResult<Vec<RenderJob>> {
    validate_render_options(&options)?;
    blocking(&engine, move |e| {
        e.enqueue_analysis_render(&demo_id, selection, options)
            .map_err(err)
    })
    .await
}

#[tauri::command]
async fn start_render(
    engine: State<'_, Eng>,
    demo_id: String,
    highlight_ids: Vec<String>,
    options: RenderOptions,
) -> CmdResult<RenderJob> {
    validate_render_options(&options)?;
    blocking(&engine, move |e| {
        e.enqueue_render(&demo_id, highlight_ids, options)
            .map_err(err)
    })
    .await
}

/// Keep native recording requests constrained even if frontend state is tampered with.
fn validate_render_options(options: &RenderOptions) -> CmdResult<()> {
    options.validate().map_err(err)
}

#[tauri::command]
async fn retry_render_job(engine: State<'_, Eng>, job_id: String) -> CmdResult<RenderJob> {
    blocking(&engine, move |e| e.retry_render_job(&job_id).map_err(err)).await
}

#[tauri::command]
async fn open_render_source(engine: State<'_, Eng>, job_id: String) -> CmdResult<DemoMeta> {
    blocking(&engine, move |e| e.open_render_source(&job_id).map_err(err)).await
}

#[tauri::command]
async fn get_clip_media(
    app: AppHandle,
    engine: State<'_, Eng>,
    job_id: String,
    output_key: String,
) -> CmdResult<demodesk_core::engine::ClipMedia> {
    blocking(&engine, move |e| {
        workspace::owned_path(e.data_dir())?;
        let media = e.get_clip_media(&job_id, &output_key).map_err(err)?;
        if let Some(path) = &media.poster_path {
            let path = workspace::owned_path(std::path::Path::new(path))?;
            app.asset_protocol_scope().allow_file(path).map_err(err)?;
        }
        Ok(media)
    })
    .await
}

#[tauri::command]
fn cancel_clip_media(engine: State<'_, Eng>, job_id: String, output_key: String) {
    engine.cancel_clip_media(&job_id, &output_key);
}

#[tauri::command]
async fn automation_status(
    app: AppHandle,
    state: State<'_, Auto>,
    refresh: bool,
) -> CmdResult<serde_json::Value> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let snapshot = if refresh {
            state.refresh()?
        } else {
            state.snapshot()
        };
        // Grant individual verified sample files, never the model/report workspace.
        if let Some(moments) = snapshot["review"]["moments"].as_array() {
            let root = workspace::root().join("state/automation/ai");
            for evidence in moments
                .iter()
                .take(2000)
                .filter_map(|m| m["evidence"].as_array())
                .flat_map(|frames| frames.iter().take(12))
            {
                if let Some(file) = evidence["file"].as_str() {
                    if let Ok(path) = approved_evidence_image(&root, file) {
                        let _ = app.asset_protocol_scope().allow_file(path);
                    }
                }
            }
        }
        Ok::<_, String>(snapshot)
    })
    .await
    .map_err(err)?
}

fn approved_evidence_image(root: &Path, file: &str) -> CmdResult<PathBuf> {
    let path = workspace::owned_path(Path::new(file))?;
    let root = workspace::owned_path(root)?;
    let relative = path.strip_prefix(&root).map_err(err)?;
    let parts: Vec<_> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let hex = |s: &str, n: usize| s.len() == n && s.bytes().all(|v| v.is_ascii_hexdigit());
    let filename = parts.get(3).map(String::as_str).unwrap_or("");
    let ordinal = filename
        .strip_prefix("source-frame-")
        .and_then(|v| v.strip_suffix(".jpg"));
    if parts.len() != 4
        || !hex(&parts[0], 64)
        || !matches!(parts[1].as_str(), "evidence-source-ordinal-v1"|"evidence-source-ordinal-v2-2fps"|"evidence-source-ordinal-v2-4fps")
        || !hex(&parts[2], 32)
        || !ordinal.is_some_and(|v| v.len() == 8 && v.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("Evidence image is outside the verified sample cache.".into());
    }
    let metadata = std::fs::metadata(&path).map_err(err)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 2 * 1024 * 1024 {
        return Err("Evidence image size is invalid.".into());
    }
    Ok(path)
}

#[tauri::command]
async fn automation_save(
    state: State<'_, Auto>,
    values: serde_json::Value,
) -> CmdResult<serde_json::Value> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || state.save(values))
        .await
        .map_err(err)?
}

#[tauri::command]
fn automation_action(state: State<'_, Auto>, request: serde_json::Value) -> CmdResult<()> {
    state.inner().start(request)
}

#[tauri::command]
fn automation_cancel(state: State<'_, Auto>) {
    state.cancel();
}

#[tauri::command]
async fn prepare_match_review(
    state: State<'_, Auto>,
    community: State<'_, Community>,
    demo_id: String,
    player_id: String,
    mode: Option<String>,
) -> CmdResult<()> {
    let state = state.inner().clone();
    let community = community.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let profile = community.profile(&player_id)?;
        state.prepare_review_mode_with_profile(demo_id, player_id, mode.unwrap_or_else(||"targeted".into()), profile)
    })
        .await
        .map_err(err)?
}

#[tauri::command]
async fn community_action(
    app: AppHandle,
    state: State<'_, Community>,
    action: String,
    values: serde_json::Value,
) -> CmdResult<serde_json::Value> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = state.action(&action, values)?;
        if matches!(action.as_str(), "contribution_preview"|"reviewer_open") {
            let file = result["mediaPath"].as_str().ok_or_else(||result["message"].as_str().unwrap_or("This example is unavailable.").to_string())?;
            let path = workspace::owned_path(Path::new(file))?;
            let root = workspace::root().join("state/community");
            let relative = path.strip_prefix(&root).map_err(err)?;
            let parts: Vec<_> = relative.components().map(|p|p.as_os_str().to_string_lossy().into_owned()).collect();
            if parts.len()!=3 || !matches!(parts[0].as_str(),"packages"|"reviewer")
                || parts[1].len()!=32 || !parts[1].bytes().all(|v|v.is_ascii_hexdigit())
                || parts[2]!="example.mp4" || !path.is_file() {
                return Err("This media is outside the prepared example library.".into());
            }
            app.asset_protocol_scope().allow_file(path).map_err(err)?;
        }
        if action=="contribution_export" {
            let file = result["packagePath"].as_str().ok_or("This offline package is missing.")?;
            let path = workspace::owned_path(Path::new(file))?;
            if !path.is_file() || !path.starts_with(workspace::root().join("state/community/packages")) {
                return Err("This offline package is unavailable.".into());
            }
            tauri_plugin_opener::reveal_item_in_dir(path).map_err(err)?;
        }
        Ok(result)
    }).await.map_err(err)?
}

#[tauri::command]
async fn pause_queue(engine: State<'_, Eng>, paused: bool) -> CmdResult<()> {
    blocking(&engine, move |e| e.set_queue_paused(paused).map_err(err)).await
}

#[tauri::command]
async fn start_custom_render(
    engine: State<'_, Eng>,
    demo_id: String,
    player_id: String,
    windows: Vec<[i32; 2]>,
    title: String,
    options: RenderOptions,
    origin: Option<String>,
    candidate_ids: Option<Vec<String>>,
) -> CmdResult<RenderJob> {
    validate_render_options(&options)?;
    blocking(&engine, move |e| {
        if let Some(ids) = candidate_ids.filter(|ids|!ids.is_empty()) {
            return e.enqueue_discovery_render(&demo_id, &player_id, ids, windows, title, options).map_err(err);
        }
        e.enqueue_custom_render_with_origin(
            &demo_id,
            &player_id,
            windows,
            title,
            options,
            origin.as_deref().unwrap_or("custom"),
        )
        .map_err(err)
    })
    .await
}

#[cfg(test)]
mod recording_request_tests {
    #[test]
    fn safe_defaults_work_and_injected_launch_or_unsupported_dimensions_fail() {
        let mut options = demodesk_core::render::RenderOptions::default();
        assert!(super::validate_render_options(&options).is_ok());
        options.width = 3840;
        options.height = 2160;
        options.max_size_mb = None;
        options.voice = true;
        assert!(super::validate_render_options(&options).is_ok());
        options
            .extra_launch_options
            .push("+connect example.invalid".into());
        assert!(super::validate_render_options(&options).is_err());
        options.extra_launch_options.clear();
        options.width = u32::MAX;
        assert!(super::validate_render_options(&options).is_err());
    }

    #[test]
    fn evidence_scope_accepts_only_generated_images_inside_the_sample_cache() {
        let root = super::workspace::root().join("temp").join(format!(
            "evidence-scope-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root
            .join("a".repeat(64))
            .join("evidence-source-ordinal-v1")
            .join("b".repeat(32));
        std::fs::create_dir_all(&source).unwrap();
        let image = source.join("source-frame-00000030.jpg");
        std::fs::write(&image, b"synthetic image fixture").unwrap();
        assert!(super::approved_evidence_image(&root, image.to_str().unwrap()).is_ok());
        let report = source.join("review.json");
        std::fs::write(&report, b"{}").unwrap();
        assert!(super::approved_evidence_image(&root, report.to_str().unwrap()).is_err());
        let outside = root.parent().unwrap().join("evidence-outside-fixture.jpg");
        assert!(super::approved_evidence_image(&root, outside.to_str().unwrap()).is_err());
        let parent = source.join("../source-frame-00000030.jpg");
        assert!(super::approved_evidence_image(&root, parent.to_str().unwrap()).is_err());
        assert!(root.canonicalize().unwrap().starts_with(
            super::workspace::root()
                .join("temp")
                .canonicalize()
                .unwrap()
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tauri::command]
async fn list_jobs(engine: State<'_, Eng>) -> CmdResult<Vec<RenderJob>> {
    blocking(&engine, |e| e.list_jobs().map_err(err)).await
}

#[tauri::command]
async fn cancel_job(engine: State<'_, Eng>, id: String) -> CmdResult<bool> {
    blocking(&engine, move |e| Ok(e.cancel_job(&id))).await
}

#[tauri::command]
async fn delete_job(engine: State<'_, Eng>, id: String) -> CmdResult<()> {
    blocking(&engine, move |e| e.delete_job(&id).map_err(err)).await
}

#[tauri::command]
async fn clear_all_clips(engine: State<'_, Eng>) -> CmdResult<u64> {
    blocking(&engine, |e| e.clear_all_clips().map_err(err)).await
}

#[tauri::command]
async fn reveal_path(path: String) -> CmdResult<()> {
    let target = Path::new(&path);
    if !target.is_absolute() || !target.exists() {
        return Err("Choose an existing absolute path.".into());
    }
    tauri_plugin_opener::reveal_item_in_dir(&path).map_err(err)
}

#[tauri::command]
async fn open_path(path: String) -> CmdResult<()> {
    let target = Path::new(&path);
    if !target.is_absolute() || !target.exists() {
        return Err("Choose an existing absolute path.".into());
    }
    if !target.is_dir() {
        workspace::owned_path(target)?;
        let extension = target
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if ![
            "mp4", "mkv", "webm", "mov", "avi", "wav", "ogg", "flac", "png", "jpg", "jpeg", "txt",
            "json", "pdf",
        ]
        .contains(&extension.as_str())
        {
            return Err("Open folders and exported media only.".into());
        }
    }
    tauri_plugin_opener::open_path(&path, None::<&str>).map_err(err)
}

/// Open a web page in the default browser (only https://github.com/… is ever passed).
#[tauri::command]
async fn open_url(url: String) -> CmdResult<()> {
    let target = tauri::Url::parse(&url).map_err(err)?;
    if target.scheme() != "https"
        || !target.username().is_empty()
        || target.password().is_some()
        || !matches!(
            target.host_str(),
            Some("github.com" | "learn.microsoft.com" | "developer.microsoft.com")
        )
    {
        return Err("Only approved HTTPS documentation and source links can open.".into());
    }
    tauri_plugin_opener::open_url(&url, None::<&str>).map_err(err)
}

struct StartupError(Option<String>);

#[tauri::command]
fn get_startup_error(state: State<'_, StartupError>) -> Option<String> {
    state.0.clone()
}

#[tauri::command]
fn preferences_need_reset(directory: State<'_, Directory>) -> bool {
    directory.reset_preferences()
}

#[tauri::command]
fn acknowledge_preferences_reset(directory: State<'_, Directory>) -> CmdResult<()> {
    directory.acknowledge_preferences()
}

#[tauri::command]
fn retry_startup(app: AppHandle, state: State<'_, StartupError>) -> CmdResult<()> {
    if state.0.is_none() {
        return Err("The app is already running.".into());
    }
    app.request_restart();
    Ok(())
}

#[tauri::command]
fn recover_data_directory(
    app: AppHandle,
    directory: State<'_, Directory>,
    state: State<'_, StartupError>,
    path: Option<String>,
) -> CmdResult<()> {
    if state.0.is_none() {
        return Err("Recovery is only available before startup.".into());
    }
    directory.save(directory.validate(path)?)?;
    // Deliver Exit so the single-instance plugin releases its lock before relaunch.
    app.request_restart();
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Err(error) = workspace::initialize_environment() {
        eprintln!("ClipForge cannot initialize its workspace: {error}");
        return;
    }
    if let Some(ok) = data_directory::probe_command() {
        std::process::exit(if ok { 0 } else { 2 });
    }
    #[cfg(windows)]
    if !webview_runtime::ready() {
        return;
    }
    let builder = tauri::Builder::default();
    // Register first: duplicate launches must exit before opening the data store.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    }));
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let base = workspace::root().join("data/bootstrap");
            let config = base.join("data-directory.json");
            let default = workspace::root().join("data/clipforge");
            let (directory, initialized) = match DataDirectory::load(config.clone(), &default) {
                Ok(mut directory) => {
                    let result = directory.initialize();
                    (directory, result)
                }
                Err(error) => (DataDirectory::recovery(config, default), Err(error)),
            };
            let directory = Arc::new(directory);
            let data_dir = directory.active.clone();
            let startup = initialized.and_then(|_| {
                app.asset_protocol_scope()
                    .allow_directory(&data_dir, true)
                    .map_err(err)?;
                Engine::new(data_dir.clone(), Arc::new(TauriNotify(handle.clone()))).map_err(err)
            });
            let error = match startup {
                Ok(engine) => {
                    app.manage(automation::Automation::new(engine.clone()));
                    app.manage(community::Community::new(engine.clone()));
                    app.manage(engine);
                    None
                }
                Err(error) => Some(error),
            };
            app.manage(StartupError(error));
            app.manage(directory);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            browse_directory,
            check_for_updates,
            get_startup_error,
            preferences_need_reset,
            acknowledge_preferences_reset,
            retry_startup,
            recover_data_directory,
            get_status,
            get_settings,
            get_storage_bytes,
            save_settings,
            preview_settings,
            run_setup,
            cancel_setup,
            check_tools,
            tool_diagnostics,
            check_tool_update,
            list_demos,
            register_demo,
            parse_demo,
            get_demo,
            score_match,
            analysis_jobs,
            scoring_history,
            get_kills,
            get_replay,
            get_map_assets,
            clear_radar,
            clear_analysis,
            clear_match_anomaly,
            clear_all_analysis,
            clear_anomaly_data,
            remove_demo,
            start_render,
            retry_render_job,
            open_render_source,
            get_clip_media,
            cancel_clip_media,
            start_custom_render,
            automation_status,
            automation_save,
            automation_action,
            automation_cancel,
            prepare_match_review,
            community_action,
            pause_queue,
            analysis_clips,
            start_analysis_render,
            list_jobs,
            cancel_job,
            delete_job,
            clear_all_clips,
            reveal_path,
            open_path,
            open_url
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                if let Some(automation) = app.try_state::<Auto>() {
                    automation.shutdown();
                }
                if let Some(community) = app.try_state::<Community>() {
                    community.shutdown();
                }
                if let Some(engine) = app.try_state::<Eng>() {
                    engine.shutdown();
                }
                if let Some(directory) = app.try_state::<Directory>() {
                    directory.release_locks();
                }
            }
        });
}
