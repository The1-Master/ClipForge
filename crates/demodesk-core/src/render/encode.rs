//! FFmpeg helpers that run after HLAE has produced the raw clip: probing,
//! audio muxing, concatenation and — the part messaging apps care about —
//! re-encoding so the file lands under a byte budget (two-pass bitrate targeting).

use super::paths::to_forward_slashes;
use super::process::ProcessTree;
use super::record::ClipOutput;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub struct Control<'a> {
    cancel: &'a AtomicBool,
    log: &'a mut dyn FnMut(String),
    check_interval: Duration,
}

impl<'a> Control<'a> {
    pub fn new(cancel: &'a AtomicBool, log: &'a mut dyn FnMut(String)) -> Self {
        Self {
            cancel,
            log,
            check_interval: Duration::from_secs(5),
        }
    }

    pub fn check_cancelled(&self) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("cancelled"));
        }
        Ok(())
    }
}

fn stalled(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::TimedOut)
}

fn retry_stalled<T>(
    control: &mut Control<'_>,
    mut attempt: impl FnMut(&mut Control<'_>) -> Result<T>,
) -> Result<T> {
    for retry in 0..=3 {
        control.check_cancelled()?;
        let result = attempt(control);
        control.check_cancelled()?;
        match result {
            Err(error) if stalled(&error) => {
                (control.log)(format!("encoding stalled: {error:#}"));
                if retry == 3 {
                    return Err(
                        error.context("encoding stalled after 3 retries; source video retained")
                    );
                }
                (control.log)(format!(
                    "restarting this video's encoding from source: retry {}/3",
                    retry + 1
                ));
            }
            result => return result,
        }
    }
    unreachable!()
}

#[cfg(test)]
fn run(exe: &Path, args: &[String]) -> Result<()> {
    run_progress(
        exe,
        args,
        1.0,
        &mut |_| {},
        &mut Control::new(&AtomicBool::new(false), &mut |_| {}),
    )
}

fn run_progress(
    exe: &Path,
    args: &[String],
    seconds: f64,
    progress: &mut dyn FnMut(f64),
    control: &mut Control<'_>,
) -> Result<()> {
    control.check_cancelled()?;
    let mut cmd = Command::new(exe);
    cmd.args(["-progress", "pipe:1", "-nostats"]);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let output = args
        .last()
        .map(Path::new)
        .filter(|p| *p != Path::new("NUL") && *p != Path::new("/dev/null"));
    let out = monitored_output(&mut cmd, seconds, progress, control, output)?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(5).collect();
        return Err(anyhow!(
            "{} failed: {}",
            exe.file_name().unwrap().to_string_lossy(),
            tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
        ));
    }
    Ok(())
}

fn monitored_output(
    cmd: &mut Command,
    seconds: f64,
    progress: &mut dyn FnMut(f64),
    control: &mut Control<'_>,
    output: Option<&Path>,
) -> Result<std::process::Output> {
    monitored_output_inner(cmd, seconds, progress, control, output, None)
}

#[derive(Default)]
struct DecodedTimestampProgress {
    pending: String,
    bytes: usize,
    last_pts: Option<f64>,
}

impl DecodedTimestampProgress {
    fn observe(&mut self, chunk: &[u8]) -> std::io::Result<bool> {
        // Match the recording metadata's existing size bound. This is a probe
        // of decoded observations, not a reason to accept an oversized document.
        self.bytes = self.bytes.saturating_add(chunk.len());
        if self.bytes > 128 * 1024 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Decoded-frame probe exceeds its safe metadata size; source retained",
            ));
        }
        self.pending.push_str(&String::from_utf8_lossy(chunk));
        let mut advanced = false;
        while let Some(end) = self.pending.find('\n') {
            let line = self.pending[..end].trim();
            let entry = line.strip_suffix(',').unwrap_or(line).trim_end();
            if entry.starts_with("\"best_effort_timestamp_time\"") {
                if let Ok(value) =
                    serde_json::from_str::<serde_json::Value>(&format!("{{{entry}}}"))
                {
                    let timestamp = value
                        .as_object()
                        .filter(|object| object.len() == 1)
                        .and_then(|object| object.get("best_effort_timestamp_time"))
                        .and_then(serde_json::Value::as_str)
                        .and_then(|time| time.parse::<f64>().ok())
                        .filter(|time| time.is_finite() && *time >= 0.0);
                    if let Some(time) =
                        timestamp.filter(|time| self.last_pts.is_none_or(|last| *time > last))
                    {
                        self.last_pts = Some(time);
                        advanced = true;
                    }
                }
            }
            self.pending.drain(..=end);
        }
        Ok(advanced)
    }
}

fn monitored_frame_probe_output(
    cmd: &mut Command,
    control: &mut Control<'_>,
) -> Result<std::process::Output> {
    monitored_output_inner(
        cmd,
        1.0,
        &mut |_| {},
        control,
        None,
        Some(&mut DecodedTimestampProgress::default()),
    )
}

fn monitored_output_inner(
    cmd: &mut Command,
    seconds: f64,
    progress: &mut dyn FnMut(f64),
    control: &mut Control<'_>,
    output: Option<&Path>,
    mut timestamps: Option<&mut DecodedTimestampProgress>,
) -> Result<std::process::Output> {
    control.check_cancelled()?;
    let mut pending = String::new();
    let mut checked_at = Instant::now();
    let mut active = false;
    let mut idle_checks = 0;
    let file_state = || {
        output
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| (m.len(), m.modified().ok()))
    };
    let mut previous_file = file_state();
    let mut counters = [0.0_f64; 2];
    Ok(ProcessTree::new()?.output_with_progress(cmd, &mut |chunk| {
        if control.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ));
        }
        let current_file = file_state();
        active |= current_file != previous_file;
        previous_file = current_file;
        if let Some(timestamps) = timestamps.as_deref_mut() {
            active |= timestamps.observe(chunk)?;
        } else {
        pending.push_str(&String::from_utf8_lossy(chunk));
        while let Some(end) = pending.find('\n') {
            if let Some((key, value)) = pending[..end].trim().split_once('=') {
                if let Some(index) = ["frame", "total_size"]
                    .iter()
                    .position(|k| *k == key)
                {
                    if let Ok(value) = value.parse::<f64>() {
                        if value.is_finite() && value > counters[index] {
                            counters[index] = value;
                            active = true;
                        }
                    }
                }
            }
            if let Some(micros) = pending[..end]
                .trim()
                .strip_prefix("out_time_us=")
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite())
            {
                progress((micros / 1_000_000.0 / seconds).clamp(0.0, 1.0));
            }
            pending.drain(..=end);
        }
        }
        if checked_at.elapsed() >= control.check_interval {
            idle_checks = if active { 0 } else { idle_checks + 1 };
            active = false;
            checked_at = Instant::now();
        }
        // ponytail: six quiet windows tolerate bursty writes; tune with slow-machine traces.
        if idle_checks >= 6 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "media process had no verified frame, decoded timestamp, output size or output file changes for six {}-second checks (frame={}, reported_bytes={}, file_bytes={})",
                    control.check_interval.as_secs_f64(),
                    counters[0],
                    counters[1],
                    previous_file.as_ref().map_or(0, |(bytes, _)| *bytes)
                ),
            ));
        }
        Ok(())
    })?)
}

fn s(v: &str) -> String {
    v.to_string()
}

pub fn ffprobe_exe(ffmpeg_exe: &Path) -> PathBuf {
    ffmpeg_exe.parent().unwrap().join(if cfg!(windows) {
        "ffprobe.exe"
    } else {
        "ffprobe"
    })
}

fn probe_media(ffmpeg_exe: &Path, file: &Path, control: &mut Control<'_>) -> Result<(f64, bool)> {
    control.check_cancelled()?;
    let mut cmd = Command::new(ffprobe_exe(ffmpeg_exe));
    cmd.args([
        "-v",
        "error",
        "-show_entries",
        "format=duration:stream=codec_type",
        "-of",
        "json",
    ])
    .arg(file);
    let out = monitored_output(&mut cmd, 1.0, &mut |_| {}, control, None)?;
    if !out.status.success() {
        return Err(anyhow!("cannot probe input video"));
    }
    let info: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let seconds = info["format"]["duration"]
        .as_str()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| anyhow!("input video has no valid duration"))?;
    let audio = info["streams"]
        .as_array()
        .is_some_and(|streams| streams.iter().any(|stream| stream["codec_type"] == "audio"));
    Ok((seconds, audio))
}

pub fn probe_duration_seconds(ffmpeg_exe: &Path, file: &Path) -> Result<f64> {
    Ok(probe_media(
        ffmpeg_exe,
        file,
        &mut Control::new(&AtomicBool::new(false), &mut |_| {}),
    )?
    .0)
}

fn capture_frame_rate(stream: &serde_json::Value) -> Option<f64> {
    stream["avg_frame_rate"]
        .as_str()
        .and_then(|rate| rate.split_once('/'))
        .and_then(|(n, d)| Some(n.parse::<f64>().ok()? / d.parse::<f64>().ok()?))
}

fn decoded_video_count_output(
    command: &mut Command,
    seconds: f64,
    control: &mut Control<'_>,
) -> Result<u64> {
    let output = monitored_output(command, seconds, &mut |_| {}, control, None)?;
    anyhow::ensure!(
        output.status.success(),
        "CS2 capture's decoded video frames cannot be counted; the source was retained."
    );
    let progress = String::from_utf8_lossy(&output.stdout);
    let completed = progress
        .lines()
        .filter_map(|line| line.trim().strip_prefix("progress="))
        .last()
        == Some("end");
    let frames = progress
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("frame=")?
                .trim()
                .parse::<u64>()
                .ok()
        })
        .last();
    anyhow::ensure!(
        completed,
        "CS2 capture's video count did not report completion; the source was retained."
    );
    frames
        .filter(|frames| *frames > 0)
        .ok_or_else(|| anyhow!("CS2 capture has no decoded video frames; the source was retained."))
}

fn validate_capture_stream(
    stream: &serde_json::Value,
    width: u32,
    height: u32,
    fps: u32,
    seconds: f64,
    tick_rate: f64,
) -> Result<()> {
    let actual_width = stream["width"].as_u64().unwrap_or(0);
    let actual_height = stream["height"].as_u64().unwrap_or(0);
    let rate = capture_frame_rate(stream);
    if actual_width != u64::from(width)
        || actual_height != u64::from(height)
        || !rate.is_some_and(|rate| rate.is_finite() && (rate - f64::from(fps)).abs() < 0.001)
    {
        return Err(anyhow!("CS2 capture is {}×{} at {} FPS; requested {}×{} at {} FPS. The source was retained; no upscale was applied.",
            actual_width,actual_height,rate.map_or_else(||"unknown".into(),|rate|format!("{rate:.3}")),width,height,fps));
    }
    let actual_seconds = stream["duration"]
        .as_str()
        .and_then(|seconds| seconds.parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .or_else(|| {
            // Counted captures never use the container duration (which can
            // include a longer audio tail). The rate passed the format gate.
            let frames = stream["nb_read_frames"].as_str()?.parse::<u64>().ok()?;
            (frames > 0).then(|| frames as f64 / rate.unwrap())
        });
    let tolerance = 2.0 / f64::from(fps) + 2.0 / tick_rate;
    if !actual_seconds.is_some_and(|actual| {
        actual.is_finite() && actual > 0.0 && (actual - seconds).abs() <= tolerance
    }) {
        return Err(anyhow!("CS2 capture is {} seconds; requested {seconds:.3} seconds. The encoder output does not match the selected moment; the source was retained and no finished clip was published for this capture.",
            actual_seconds.map_or_else(|| "unknown".into(), |value| format!("{value:.3}"))));
    }
    Ok(())
}

/// Verify native capture before creating a finished export. Never resize or change FPS to hide a failed request.
pub fn validate_native_capture(
    ffmpeg: &Path,
    file: &Path,
    width: u32,
    height: u32,
    fps: u32,
    seconds: f64,
    tick_rate: f64,
    control: &mut Control<'_>,
) -> Result<()> {
    let mut command = Command::new(ffprobe_exe(ffmpeg));
    command
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,avg_frame_rate,duration:format=format_name",
            "-of",
            "json",
        ])
        .arg(file);
    let out = monitored_output(&mut command, 1.0, &mut |_| {}, control, None)?;
    if !out.status.success() {
        return Err(anyhow!(
            "CS2 capture cannot be read; the retained source may be incomplete."
        ));
    }
    let mut info: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let matroska = info["format"]["format_name"]
        .as_str()
        .is_some_and(|formats| {
            formats
                .split(',')
                .any(|format| format == "matroska" || format == "webm")
        });
    let stream = info
        .get_mut("streams")
        .and_then(serde_json::Value::as_array_mut)
        .and_then(|streams| streams.first_mut())
        .ok_or_else(|| {
            anyhow!("CS2 capture has no readable video stream; the source was retained.")
        })?;
    // Matroska can omit video duration or expose a duration inferred from a
    // longer audio track. Always measure its video; preserve the MP4 fast path.
    if matroska {
        stream["duration"] = serde_json::Value::Null;
    }
    if !stream["duration"]
        .as_str()
        .and_then(|value| value.parse::<f64>().ok())
        .is_some_and(|seconds| seconds.is_finite() && seconds > 0.0)
        && stream["width"].as_u64() == Some(u64::from(width))
        && stream["height"].as_u64() == Some(u64::from(height))
        && capture_frame_rate(stream)
            .is_some_and(|rate| rate.is_finite() && (rate - f64::from(fps)).abs() < 0.001)
    {
        // Decode to a null sink with actual frame progress. FFprobe's EOF-only
        // count would falsely trip the stall guard for a longer valid capture.
        let mut count = Command::new(ffmpeg);
        count
            .args([
                "-v",
                "error",
                "-nostdin",
                "-nostats",
                "-progress",
                "pipe:1",
                "-protocol_whitelist",
                "file",
                "-format_whitelist",
                "mov,matroska",
                "-threads",
                "1",
                "-i",
            ])
            .arg(file)
            .args([
                "-map",
                "0:v:0",
                "-an",
                "-sn",
                "-dn",
                "-threads",
                "1",
                "-fps_mode",
                "passthrough",
                "-f",
                "null",
            ])
            .arg(if cfg!(windows) { "NUL" } else { "/dev/null" });
        let decoded = decoded_video_count_output(&mut count, seconds, control)?;
        stream["nb_read_frames"] = json_value(decoded.to_string());
    }
    validate_capture_stream(&info["streams"][0], width, height, fps, seconds, tick_rate).map_err(
        |error| match super::diagnostics::capture_encoder_failure(file) {
            Some(diagnostic) => anyhow!("{error:#} {diagnostic}"),
            None => error,
        },
    )
}

pub fn mux_clip(
    ffmpeg_exe: &Path,
    clip: &ClipOutput,
    dest: &Path,
    audio_kbps: u32,
    control: &mut Control<'_>,
) -> Result<()> {
    let video = clip
        .video
        .as_ref()
        .ok_or_else(|| anyhow!("clip {}: no video file", clip.index + 1))?;
    let mut args = vec![s("-y"), s("-i"), video.to_string_lossy().to_string()];
    if let Some(audio) = &clip.audio {
        args.extend([
            s("-i"),
            audio.to_string_lossy().to_string(),
            s("-map"),
            s("0:v:0"),
            s("-map"),
            s("1:a:0"),
            s("-c:v"),
            s("copy"),
            s("-c:a"),
            s("aac"),
            s("-b:a"),
            format!("{audio_kbps}k"),
            s("-ar"),
            s("48000"),
            s("-ac"),
            s("2"),
            // Audio can stop a few samples before the last video frame. Keep
            // every observed capture ordinal; pad only audio through video EOF.
            s("-af"),
            s("apad"),
            s("-shortest"),
        ]);
    } else {
        args.extend([s("-c:v"), s("copy")]);
    }
    args.extend([
        s("-avoid_negative_ts"),
        s("disabled"),
        s("-movflags"),
        s("+faststart"),
    ]);
    args.push(dest.to_string_lossy().to_string());
    run_progress(ffmpeg_exe, &args, 1.0, &mut |_| {}, control)?;
    publish_timeline(ffmpeg_exe, video, dest, control)
}

fn publish_timeline(
    ffmpeg: &Path,
    source: &Path,
    dest: &Path,
    control: &mut Control<'_>,
) -> Result<()> {
    let sidecar = timeline_path(source);
    if !sidecar.is_file() {
        return Ok(());
    }
    publish_timeline_document(ffmpeg, read_timeline(&sidecar)?, dest, control)
}

fn timeline_path(video: &Path) -> PathBuf {
    PathBuf::from(format!("{}.timeline.json", video.display()))
}
fn read_timeline(path: &Path) -> Result<serde_json::Value> {
    anyhow::ensure!(
        std::fs::metadata(path)?.len() <= 128 * 1024 * 1024,
        "Capture timeline exceeds its safe metadata size."
    );
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
fn video_hash(path: &Path, control: &Control<'_>) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut chunk = [0u8; 65536];
    loop {
        control.check_cancelled()?;
        let n = file.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        hash.update(&chunk[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn timeline_matches_video(timeline: &serde_json::Value, info: &serde_json::Value) -> bool {
    let video = &info["streams"][0];
    let count = video["nb_read_frames"]
        .as_str()
        .and_then(|v| v.parse::<u64>().ok());
    let fps = timeline["fps"]
        .as_u64()
        .filter(|fps| (1..=240).contains(fps));
    let rate = video["avg_frame_rate"]
        .as_str()
        .and_then(|v| v.split_once('/'))
        .and_then(|(n, d)| Some(n.parse::<f64>().ok()? / d.parse::<f64>().ok()?));
    let Some(frames) = timeline["frames"].as_array() else {
        return false;
    };
    let Some(actual) = info["frames"].as_array() else {
        return false;
    };
    let Some(fps) = fps else {
        return false;
    };
    timeline["schemaVersion"] == 1
        && timeline["mapping"] == "HLAE_FRAME_RENDER_PASS_ordinal"
        && timeline["captureBoundary"] == "CStream::EngineThread_EndFrame afterCommands"
        && !frames.is_empty()
        && frames.len() <= 2_000_000
        && timeline["frameCount"].as_u64() == Some(frames.len() as u64)
        && count == Some(frames.len() as u64)
        && actual.len() == frames.len()
        && rate.is_some_and(|rate| rate.is_finite() && (rate - fps as f64).abs() < 0.001)
        && frames
            .iter()
            .zip(actual)
            .enumerate()
            .all(|(index, (frame, encoded))| {
                let expected = index as f64 / fps as f64;
                frame["frameIndex"].as_u64() == Some(index as u64)
                    && frame["demoTick"].as_i64().is_some_and(|t| t >= 0)
                    && frame["ptsSeconds"]
                        .as_f64()
                        .is_some_and(|t| t.is_finite() && (t - expected).abs() <= 0.001)
                    && encoded["best_effort_timestamp_time"]
                        .as_str()
                        .and_then(|v| v.parse::<f64>().ok())
                        .is_some_and(|t| t.is_finite() && (t - expected).abs() <= 0.001)
                    && frame["view"]["width"] == video["width"]
                    && frame["view"]["height"] == video["height"]
            })
}
fn publish_timeline_document(
    ffmpeg: &Path,
    mut timeline: serde_json::Value,
    dest: &Path,
    control: &mut Control<'_>,
) -> Result<()> {
    let mut command = Command::new(ffprobe_exe(ffmpeg));
    command
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file",
            "-format_whitelist",
            "mov,matroska",
            "-select_streams",
            "v:0",
            "-count_frames",
            "-show_frames",
            "-show_entries",
            "stream=nb_read_frames,avg_frame_rate,width,height:frame=best_effort_timestamp_time",
            "-of",
            "json",
        ])
        .arg(dest);
    let probe = monitored_frame_probe_output(&mut command, control)?;
    anyhow::ensure!(
        probe.status.success(),
        "The finished video cannot be checked against its recording timeline."
    );
    let info: serde_json::Value = serde_json::from_slice(&probe.stdout)?;
    let valid = timeline_matches_video(&timeline, &info);
    timeline["validated"] = json_value(valid);
    timeline["encodedFrameCount"] = info["streams"][0]["nb_read_frames"]
        .as_str()
        .and_then(|v| v.parse::<u64>().ok())
        .map(json_value)
        .unwrap_or(serde_json::Value::Null);
    timeline["encodedTiming"] = serde_json::json!({"avgFrameRate":info["streams"][0]["avg_frame_rate"],"firstPTS":info["frames"][0]["best_effort_timestamp_time"],"lastPTS":info["frames"].as_array().and_then(|frames|frames.last()).map(|frame|frame["best_effort_timestamp_time"].clone())});
    timeline["videoFile"] = json_value(dest.to_string_lossy().to_string());
    timeline["videoSHA256"] = json_value(video_hash(dest, control)?);
    timeline["validation"] = json_value(if valid {
        "decoded frame count, ordinal sequence, actual PTS, FPS and render dimensions match"
    } else {
        "mapping not certified; local AI must skip this video"
    });
    let bytes = serde_json::to_vec(&timeline)?;
    crate::store::write_atomic(&timeline_path(dest), &bytes)?;
    (control.log)(format!(
        "video timeline {}",
        if valid {
            "validated"
        } else {
            "could not be validated"
        }
    ));
    anyhow::ensure!(valid,"The finished video's recording timeline does not match its decoded frame count, timestamps, FPS or dimensions. The source was retained; this export was not certified.");
    Ok(())
}

/// Preserve measured observations through edits. Segment IDs prohibit AI candidates spanning a cut.
fn compose_timelines(documents: Vec<(PathBuf, serde_json::Value)>) -> Result<serde_json::Value> {
    let mut base = None;
    let mut all = Vec::new();
    let mut next_segment = 0_u64;
    let mut target = None;
    let mut mixed_target = false;
    let mut sources = Vec::new();
    let mut fps = None;
    for (path, mut doc) in documents {
        let rate = doc["fps"]
            .as_u64()
            .filter(|r| (1..=240).contains(r))
            .ok_or_else(|| anyhow!("Source timeline FPS is invalid."))?;
        anyhow::ensure!(
            fps.is_none_or(|fps| fps == rate),
            "Clip timelines have different FPS."
        );
        fps = Some(rate);
        let player = doc["targetSteamId"].clone();
        if let Some(previous) = &target {
            mixed_target |= *previous != player;
        } else {
            target = Some(player);
        }
        let serde_json::Value::Array(frames) = doc["frames"].take() else {
            anyhow::bail!("Source timeline has no observed frames.");
        };
        anyhow::ensure!(
            !frames.is_empty() && doc["frameCount"].as_u64() == Some(frames.len() as u64),
            "Source timeline frame count is invalid."
        );
        let first_frame = all.len();
        let mut local_segments = std::collections::HashMap::new();
        for (index, mut frame) in frames.into_iter().enumerate() {
            anyhow::ensure!(
                frame["frameIndex"].as_u64() == Some(index as u64),
                "Source timeline ordinals are incomplete."
            );
            let local = frame["segmentId"].as_u64().unwrap_or(0);
            let segment = *local_segments.entry(local).or_insert_with(|| {
                let id = next_segment;
                next_segment += 1;
                id
            });
            let original = frame["sourceFrameIndex"].as_u64().unwrap_or(index as u64);
            frame["sourceFrameIndex"] = json_value(original);
            frame["segmentId"] = json_value(segment);
            frame["frameIndex"] = json_value(all.len());
            frame["ptsSeconds"] = json_value(all.len() as f64 / rate as f64);
            all.push(frame);
            anyhow::ensure!(
                all.len() <= 2_000_000,
                "Combined timeline exceeds its safe observed-frame limit."
            );
        }
        sources.push(serde_json::json!({"videoFile":path,"videoSHA256":doc["videoSHA256"],"startFrame":first_frame,"endFrame":all.len(),"targetSteamId":doc["targetSteamId"]}));
        if base.is_none() {
            doc["frames"] = serde_json::Value::Null;
            doc.as_object_mut().unwrap().remove("segments");
            base = Some(doc);
        }
    }
    let mut document =
        base.ok_or_else(|| anyhow!("Choose clip files before combining timelines."))?;
    document["targetSteamId"] = if mixed_target {
        serde_json::Value::Null
    } else {
        target.unwrap_or(serde_json::Value::Null)
    };
    document["frameCount"] = json_value(all.len());
    document["frames"] = json_value(all);
    document["segments"] = json_value(sources);
    document["edited"] = json_value(true);
    document["startTick"] = document["frames"][0]["demoTick"].clone();
    document["endTick"] = document["frames"]
        .as_array()
        .and_then(|frames| frames.last())
        .map(|f| f["demoTick"].clone())
        .unwrap_or(serde_json::Value::Null);
    document["validated"] = json_value(false);
    Ok(document)
}

fn json_value(value: impl serde::Serialize) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

pub fn concat_clips(
    ffmpeg_exe: &Path,
    clips: &[PathBuf],
    dest: &Path,
    control: &mut Control<'_>,
) -> Result<()> {
    let mut documents = Vec::new();
    let mut durations = Vec::new();
    let mut have_timeline = true;
    for file in clips {
        let sidecar = timeline_path(file);
        if !sidecar.is_file() {
            have_timeline = false;
            break;
        }
        let timeline = read_timeline(&sidecar)?;
        if timeline["validated"] != true {
            have_timeline = false;
            break;
        }
        anyhow::ensure!(timeline["videoSHA256"].as_str()==Some(video_hash(file,control)?.as_str()),"A clip changed after its recording timeline was verified. Keep the source and export it again.");
        let count = timeline["frameCount"]
            .as_u64()
            .ok_or_else(|| anyhow!("Source timeline has no frame count."))?;
        let fps = timeline["fps"]
            .as_u64()
            .filter(|v| *v > 0)
            .ok_or_else(|| anyhow!("Source timeline has no valid FPS."))?;
        durations.push(count as f64 / fps as f64);
        documents.push((file.clone(), timeline));
    }
    let timeline = if have_timeline {
        Some(compose_timelines(documents)?)
    } else {
        None
    };
    let list = dest.parent().unwrap().join("concat.txt");
    let body: Vec<String> = clips
        .iter()
        .enumerate()
        .map(|(index, c)| {
            let file = format!("file '{}'", to_forward_slashes(c).replace('\'', "'\\''"));
            // Align each next video frame to measured video EOF, not AAC/container padding.
            if timeline.is_some() {
                format!("{file}\nduration {:.12}", durations[index])
            } else {
                file
            }
        })
        .collect();
    std::fs::write(&list, body.join("\n"))?;
    let r = run_progress(
        ffmpeg_exe,
        &[
            s("-y"),
            s("-protocol_whitelist"),
            s("file"),
            s("-copyts"),
            s("-format_whitelist"),
            s("concat,mov,matroska"),
            s("-f"),
            s("concat"),
            s("-safe"),
            s("0"),
            s("-i"),
            list.to_string_lossy().to_string(),
            s("-c"),
            s("copy"),
            s("-movflags"),
            s("+faststart"),
            dest.to_string_lossy().to_string(),
        ],
        1.0,
        &mut |_| {},
        control,
    );
    let _ = std::fs::remove_file(&list);
    r?;
    if let Some(timeline) = timeline {
        publish_timeline_document(ffmpeg_exe, timeline, dest, control)?;
    }
    Ok(())
}

/// Bitrate in kbit/s that fits `max_size_mb` for `seconds` of video, leaving room for audio and container overhead.
pub fn video_bitrate_for_size(max_size_mb: f64, seconds: f64, audio_kbps: u32) -> u32 {
    if !max_size_mb.is_finite() || max_size_mb <= 0.0 || !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    // MB and FFmpeg kbit/s are decimal. Reserve 2% for muxing and rate-control error.
    ((max_size_mb * 1_000_000.0 * 8.0 * 0.98 / seconds / 1000.0 - audio_kbps as f64)
        .floor()
        .max(0.0)) as u32
}

#[derive(Debug)]
pub struct SizeResult {
    pub bitrate_kbps: u32,
    pub bytes: u64,
}

/// Video codecs we know how to drive: (ffmpeg encoder name, label).
pub const CODECS: [(&str, &str); 6] = [
    ("libx264", "H.264 CPU"),
    ("libx265", "H.265 CPU"),
    ("h264_nvenc", "H.264 NVIDIA"),
    ("hevc_nvenc", "H.265 NVIDIA"),
    ("h264_amf", "H.264 AMD"),
    ("hevc_amf", "H.265 AMD"),
];

fn is_hevc(codec: &str) -> bool {
    codec == "libx265" || codec == "hevc_nvenc" || codec == "hevc_amf"
}

/// `hvc1` tag so HEVC-in-mp4 plays in QuickTime / iOS / Windows Media Player.
fn tag_args(codec: &str) -> Vec<String> {
    if is_hevc(codec) {
        vec![s("-tag:v"), s("hvc1")]
    } else {
        vec![]
    }
}

const NVENC_RECORDING: &str =
    "-preset p5 -tune hq -multipass qres -rc-lookahead 0 -spatial-aq 1 -bf 2";

const NVENC_COMPATIBLE: &str = "-preset p4 -tune hq -multipass disabled -rc-lookahead 0 -spatial-aq 0 -temporal-aq 0 -bf 0 -b_ref_mode disabled";

fn with_nvenc_fallback<T>(
    compatible: &mut bool,
    mut encode: impl FnMut(bool) -> Result<T>,
) -> Result<T> {
    match encode(*compatible) {
        Ok(value) => Ok(value),
        Err(first) if !*compatible && !stalled(&first) && first.to_string() != "cancelled" => {
            *compatible = true;
            eprintln!("NVENC failed; retrying with compatibility settings: {first:#}");
            encode(true).map_err(|last| {
                anyhow!("NVENC failed: {first:#}; compatibility retry failed: {last:#}")
            })
        }
        Err(error) => Err(error),
    }
}

/// Probe the actual capture format before launching the game, with one compatibility retry.
pub fn checked_record_preset(
    ffmpeg: &Path,
    options: &super::RenderOptions,
    log: &mut dyn FnMut(String),
    cancel: &AtomicBool,
) -> Result<(String, bool)> {
    let preset = record_preset(&options.codec, options.crf, options.fps);
    if !options.codec.contains("nvenc") && !options.codec.contains("_amf") {
        return Ok((preset, false));
    }
    let mut compatible = false;
    let mut attempt = |fallback| {
        let selected = if fallback {
            preset
                .replace(NVENC_RECORDING, NVENC_COMPATIBLE)
                .replace("main -b_ref_mode middle", "main")
        } else {
            preset.clone()
        };
        let mut args = vec![
            s("-f"),
            s("lavfi"),
            s("-i"),
            format!(
                "color=size={}x{}:rate={}",
                options.width, options.height, options.fps
            ),
            s("-frames:v"),
            s("3"),
        ];
        args.extend(selected.split_whitespace().map(s));
        args.extend([
            s("-f"),
            s("null"),
            s(if cfg!(windows) { "NUL" } else { "/dev/null" }),
        ]);
        run_progress(
            ffmpeg,
            &args,
            1.0,
            &mut |_| {},
            &mut Control::new(cancel, log),
        )?;
        Ok(selected)
    };
    let selected = if options.codec.contains("nvenc") {
        with_nvenc_fallback(&mut compatible, attempt)?
    } else {
        attempt(false)?
    };
    if compatible {
        log("NVENC: using compatibility settings (B-frames, AQ and multipass disabled)".into());
    }
    Ok((selected, compatible))
}

/// Quality-targeted arguments HLAE hands to ffmpeg while recording (`crf` ≈ quality, lower = better).
/// NVENC uses constant QP for capture; size fitting uses VBR separately.
pub fn record_preset(codec: &str, crf: u32, fps: u32) -> String {
    let mut parts = vec![format!("-c:v {codec}"), s("-pix_fmt yuv420p")];
    if codec.contains("nvenc") {
        parts.push(format!(
            "-rc constqp -qp {crf} -b:v 0 {NVENC_RECORDING} -g {} -profile:v {}",
            u64::from(fps) * 2,
            if is_hevc(codec) {
                "main -b_ref_mode middle"
            } else {
                "high"
            }
        ));
    } else if codec.contains("_amf") {
        parts.push(format!(
            "-quality quality -rc cqp -qp_i {crf} -qp_p {} -g {}",
            crf.saturating_add(2),
            u64::from(fps) * 2
        ));
    } else {
        parts.push(format!(
            "-crf {crf} -preset {} -profile:v {} -g {}",
            if is_hevc(codec) { "fast" } else { "veryfast" },
            if is_hevc(codec) { "main" } else { "high" },
            u64::from(fps) * 2
        ));
    }
    parts.extend(tag_args(codec));
    parts.join(" ")
}

/// Re-encode from the same source on each attempt; publish only a size-checked file.
/// NVENC multipass is per-frame analysis, not the CPU encoders' file-level two passes.
pub fn encode_to_size(
    ffmpeg_exe: &Path,
    input: &Path,
    output: &Path,
    max_size_mb: f64,
    codec: &str,
    audio_kbps: u32,
) -> Result<SizeResult> {
    encode_to_size_with_progress(
        ffmpeg_exe,
        input,
        output,
        SizeOptions {
            max_size_mb,
            codec,
            audio_kbps,
        },
        &mut false,
        &mut |_| {},
        &mut Control::new(&AtomicBool::new(false), &mut |_| {}),
    )
}

pub struct SizeOptions<'a> {
    pub max_size_mb: f64,
    pub codec: &'a str,
    pub audio_kbps: u32,
}

pub fn encode_to_size_with_progress(
    ffmpeg_exe: &Path,
    input: &Path,
    output: &Path,
    options: SizeOptions<'_>,
    compatible: &mut bool,
    progress: &mut dyn FnMut(f64),
    control: &mut Control<'_>,
) -> Result<SizeResult> {
    let SizeOptions {
        max_size_mb,
        codec,
        audio_kbps,
    } = options;
    let mut reported = 0.0_f64;
    retry_stalled(control, |control| {
        if !CODECS.iter().any(|(name, _)| *name == codec) {
            return Err(anyhow!("unsupported video encoder: {codec}"));
        }
        if !max_size_mb.is_finite() || max_size_mb <= 0.0 {
            return Err(anyhow!("invalid file size limit"));
        }
        if input == output {
            return Err(anyhow!("input and output must be different files"));
        }
        (control.log)(format!(
            "size fitting: {} ({codec}, limit {max_size_mb} MB)",
            input.file_name().unwrap_or_default().to_string_lossy()
        ));
        let (seconds, has_audio) = probe_media(ffmpeg_exe, input, control)?;
        let audio_kbps = if has_audio { audio_kbps } else { 0 };
        let limit = (max_size_mb * 1_000_000.0).floor() as u64;
        let mut bitrate = video_bitrate_for_size(max_size_mb, seconds, audio_kbps);
        if bitrate == 0 {
            return Err(anyhow!(
                "size limit is too small for this duration and audio bitrate"
            ));
        }
        let temp = tempfile::Builder::new()
            .prefix("encode-")
            .suffix(".mp4")
            .tempfile_in(
                output
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?
            .into_temp_path();
        let input_s = input.to_string_lossy().to_string();
        let output_s = temp.to_string_lossy().to_string();
        let mut common = vec![
            s("-y"),
            s("-i"),
            input_s.clone(),
            s("-map"),
            s("0:v:0"),
            s("-map"),
            s("0:a:0?"),
            s("-c:a"),
            s("aac"),
            s("-b:a"),
            format!("{audio_kbps}k"),
            s("-ar"),
            s("48000"),
            s("-ac"),
            s("2"),
            s("-movflags"),
            s("+faststart"),
            s("-pix_fmt"),
            s("yuv420p"),
            s("-fps_mode"),
            s("passthrough"),
        ];
        common.extend(tag_args(codec));
        for attempt in 0..4 {
            control.check_cancelled()?;
            (control.log)(format!(
                "size fitting attempt {}/4: {bitrate} kbps, {seconds:.2} seconds",
                attempt + 1
            ));
            // Leave space for size verification and retries without moving backwards.
            let start = 1.0 - 0.1_f64.powi(attempt);
            let span = 0.9 * 0.1_f64.powi(attempt);
            if codec.contains("nvenc") {
                with_nvenc_fallback(compatible, |fallback| {
                    let mut args = common.clone();
                    args.extend([
                        s("-c:v"),
                        s(codec),
                        s("-rc"),
                        s("vbr"),
                        s("-b:v"),
                        format!("{bitrate}k"),
                        s("-maxrate"),
                        format!("{bitrate}k"),
                        s("-bufsize"),
                        format!("{}k", u64::from(bitrate) * 2),
                    ]);
                    args.extend(
                        if fallback {
                            NVENC_COMPATIBLE
                        } else {
                            NVENC_RECORDING
                        }
                        .split_whitespace()
                        .map(s),
                    );
                    args.extend([
                        s("-force_key_frames"),
                        s("expr:gte(t,n_forced*2)"),
                        s("-profile:v"),
                        s(if is_hevc(codec) { "main" } else { "high" }),
                    ]);
                    if is_hevc(codec) && !fallback {
                        args.extend([s("-b_ref_mode"), s("middle")]);
                    }
                    args.push(output_s.clone());
                    run_progress(
                        ffmpeg_exe,
                        &args,
                        seconds,
                        &mut |p| {
                            reported = reported.max(start + span * p);
                            progress(reported);
                        },
                        control,
                    )
                })?;
            } else if codec.contains("_amf") {
                let mut args = common.clone();
                args.extend([
                    s("-c:v"),
                    s(codec),
                    s("-quality"),
                    s("quality"),
                    s("-rc"),
                    s("cbr"),
                    s("-b:v"),
                    format!("{bitrate}k"),
                    s("-maxrate"),
                    format!("{bitrate}k"),
                    s("-bufsize"),
                    format!("{}k", u64::from(bitrate) * 2),
                    s("-force_key_frames"),
                    s("expr:gte(t,n_forced*2)"),
                ]);
                args.push(output_s.clone());
                run_progress(
                    ffmpeg_exe,
                    &args,
                    seconds,
                    &mut |p| {
                        reported = reported.max(start + span * p);
                        progress(reported);
                    },
                    control,
                )?;
            } else {
                let tmp = tempfile::tempdir()?;
                let passlog = tmp
                    .path()
                    .join("ffmpeg2pass")
                    .to_string_lossy()
                    .replace('\\', "/");
                let null_sink = if cfg!(windows) { "NUL" } else { "/dev/null" };
                let rate = [
                    s("-c:v"),
                    s(codec),
                    s("-b:v"),
                    format!("{bitrate}k"),
                    s("-maxrate"),
                    format!("{}k", (bitrate as f64 * 1.3) as u32),
                    s("-bufsize"),
                    format!("{}k", u64::from(bitrate) * 2),
                    s("-preset"),
                    s("medium"),
                    s("-pix_fmt"),
                    s("yuv420p"),
                    s("-fps_mode"),
                    s("passthrough"),
                ];
                let pass_args = |n: u32| -> Vec<String> {
                    if codec == "libx265" {
                        vec![
                            s("-x265-params"),
                            format!("pass={n}:stats={}.x265", passlog.replace(':', "\\:")),
                        ]
                    } else {
                        vec![
                            s("-pass"),
                            n.to_string(),
                            s("-passlogfile"),
                            passlog.clone(),
                        ]
                    }
                };
                let mut pass1 = vec![s("-y"), s("-i"), input_s.clone(), s("-map"), s("0:v:0")];
                pass1.extend(rate.iter().cloned());
                pass1.extend(pass_args(1));
                pass1.extend([s("-an"), s("-f"), s("null"), s(null_sink)]);
                (control.log)("size fitting: CPU pass 1/2".into());
                run_progress(
                    ffmpeg_exe,
                    &pass1,
                    seconds,
                    &mut |p| {
                        reported = reported.max(start + span * p / 2.0);
                        progress(reported)
                    },
                    control,
                )?;
                let mut pass2 = common.clone();
                pass2.extend(rate.iter().cloned());
                pass2.extend(pass_args(2));
                pass2.push(output_s.clone());
                (control.log)("size fitting: CPU pass 2/2".into());
                run_progress(
                    ffmpeg_exe,
                    &pass2,
                    seconds,
                    &mut |p| {
                        reported = reported.max(start + span * (0.5 + p / 2.0));
                        progress(reported)
                    },
                    control,
                )?;
            }
            control.check_cancelled()?;
            let bytes = std::fs::metadata(&temp)?.len();
            (control.log)(format!(
                "size fitting result: {bytes} bytes (limit {limit})"
            ));
            if bytes > 0 && bytes <= limit {
                temp.persist(output)?;
                publish_timeline(ffmpeg_exe, input, output, control)?;
                progress(1.0);
                return Ok(SizeResult {
                    bitrate_kbps: bitrate,
                    bytes,
                });
            }
            let audio_bytes = audio_kbps as f64 * 1000.0 * seconds / 8.0;
            let ratio = ((limit as f64 - audio_bytes) / (bytes as f64 - audio_bytes).max(1.0)
                * 0.97)
                .clamp(0.0, 0.95);
            bitrate = (bitrate as f64 * ratio).floor() as u32;
            if bitrate == 0 {
                break;
            }
        }
        Err(anyhow!("could not meet {max_size_mb} MB without changing resolution, FPS or encoder; source video was retained"))
    })
}

pub fn bytes_to_mb(bytes: u64) -> f64 {
    (bytes as f64 / 1_000_000.0 * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timeline_fixture(count: usize, start_tick: i32) -> serde_json::Value {
        serde_json::json!({"schemaVersion":1,"mapping":"HLAE_FRAME_RENDER_PASS_ordinal","captureBoundary":"CStream::EngineThread_EndFrame afterCommands","fps":30,"targetSteamId":"fixture-player","validated":false,"frameCount":count,"frames":(0..count).map(|n|serde_json::json!({"frameIndex":n,"ptsSeconds":n as f64/30.0,"demoTick":start_tick+n as i32*2,"source":"source2_observer_handle","steamId":"fixture-player","alive":true,"observerMode":2,"view":{"width":320,"height":180}})).collect::<Vec<_>>()})
    }

    #[test]
    fn edited_timelines_keep_source_observations_and_distinct_cut_ids() {
        let mut a = timeline_fixture(2, 100);
        a["videoSHA256"] = json_value("a".repeat(64));
        let mut b = timeline_fixture(2, 1000);
        b["videoSHA256"] = json_value("b".repeat(64));
        let merged = compose_timelines(vec![
            (PathBuf::from("first.mp4"), a),
            (PathBuf::from("second.mp4"), b),
        ])
        .unwrap();
        assert_eq!(merged["frames"][2]["frameIndex"], 2);
        assert_eq!(merged["frames"][2]["demoTick"], 1000);
        assert_eq!(merged["frames"][2]["sourceFrameIndex"], 0);
        assert_ne!(
            merged["frames"][1]["segmentId"],
            merged["frames"][2]["segmentId"]
        );
        let mut invalid = timeline_fixture(2, 100);
        invalid["frames"][1]["frameIndex"] = json_value(3);
        assert!(compose_timelines(vec![(PathBuf::from("invalid.mp4"), invalid)]).is_err());
    }

    #[test]
    fn timeline_certification_rejects_equal_frame_count_with_wrong_actual_pts() {
        let mut timeline = timeline_fixture(2, 100);
        let mut info = serde_json::json!({"streams":[{"nb_read_frames":"2","avg_frame_rate":"30/1","width":320,"height":180}],"frames":[{"best_effort_timestamp_time":"0"},{"best_effort_timestamp_time":"0.033333"}]});
        assert!(timeline_matches_video(&timeline, &info));
        info["frames"][1]["best_effort_timestamp_time"] = json_value("0.050000");
        assert!(!timeline_matches_video(&timeline, &info));
        info["frames"][1]["best_effort_timestamp_time"] = json_value("0.033333");
        timeline["frames"][1]["ptsSeconds"] = json_value(0.05);
        assert!(!timeline_matches_video(&timeline, &info));
    }

    #[test]
    #[ignore = "requires bundled CPU FFmpeg via CLIPFORGE_TEST_FFMPEG"]
    fn actual_mux_concat_size_fit_and_mkv_keep_certified_frame_mapping() {
        let ffmpeg = PathBuf::from(std::env::var_os("CLIPFORGE_TEST_FFMPEG").unwrap());
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().to_path_buf();
        let _cleanup = if std::env::var_os("CLIPFORGE_TEST_KEEP_TIMELINE_FIXTURE").is_some() {
            println!("Synthetic timeline fixture {}", temporary.keep().display());
            None
        } else {
            Some(temporary)
        };
        let root = path.as_path();
        let raw = root.join("raw.mp4");
        let audio = root.join("audio.wav");
        run(
            &ffmpeg,
            &[
                s("-y"),
                s("-f"),
                s("lavfi"),
                s("-i"),
                s("testsrc2=size=320x180:rate=30"),
                s("-frames:v"),
                s("30"),
                s("-c:v"),
                s("libx264"),
                s("-threads"),
                s("1"),
                s("-an"),
                raw.to_string_lossy().into_owned(),
            ],
        )
        .unwrap();
        run(
            &ffmpeg,
            &[
                s("-y"),
                s("-f"),
                s("lavfi"),
                s("-i"),
                s("sine=frequency=600:sample_rate=48000"),
                s("-t"),
                s("0.98"),
                s("-ac"),
                s("2"),
                audio.to_string_lossy().into_owned(),
            ],
        )
        .unwrap();
        let mut paths = Vec::new();
        let cancelled = AtomicBool::new(false);
        let mut log = |_: String| {};
        for (n, start) in [100, 10000].into_iter().enumerate() {
            std::fs::write(
                timeline_path(&raw),
                serde_json::to_vec(&timeline_fixture(30, start)).unwrap(),
            )
            .unwrap();
            let output = root.join(format!("mux-{n}.mp4"));
            mux_clip(
                &ffmpeg,
                &ClipOutput {
                    index: n,
                    video: Some(raw.clone()),
                    audio: Some(audio.clone()),
                },
                &output,
                64,
                &mut Control::new(&cancelled, &mut log),
            )
            .unwrap();
            assert_eq!(
                read_timeline(&timeline_path(&output)).unwrap()["validated"],
                true
            );
            paths.push(output);
        }
        let merged = root.join("merged.mp4");
        concat_clips(
            &ffmpeg,
            &paths,
            &merged,
            &mut Control::new(&cancelled, &mut log),
        )
        .unwrap();
        let timeline = read_timeline(&timeline_path(&merged)).unwrap();
        assert_eq!(timeline["validated"], true);
        assert_eq!(timeline["encodedFrameCount"], 60);
        assert_eq!(timeline["frames"][30]["demoTick"], 10000);
        assert_eq!(timeline["frames"][30]["ptsSeconds"], 1.0);
        assert_ne!(
            timeline["frames"][29]["segmentId"],
            timeline["frames"][30]["segmentId"]
        );
        let fitted = root.join("fitted.mp4");
        encode_to_size_with_progress(
            &ffmpeg,
            &merged,
            &fitted,
            SizeOptions {
                max_size_mb: 0.04,
                codec: "libx264",
                audio_kbps: 64,
            },
            &mut false,
            &mut |_| {},
            &mut Control::new(&cancelled, &mut log),
        )
        .unwrap();
        let fitted_timeline = read_timeline(&timeline_path(&fitted)).unwrap();
        assert_eq!(fitted_timeline["validated"], true);
        assert_eq!(fitted_timeline["encodedFrameCount"], 60);
        assert!(std::fs::metadata(&fitted).unwrap().len() <= 40000);
        let mkv = root.join("mux.mkv");
        mux_clip(
            &ffmpeg,
            &ClipOutput {
                index: 0,
                video: Some(raw.clone()),
                audio: Some(audio),
            },
            &mkv,
            64,
            &mut Control::new(&cancelled, &mut log),
        )
        .unwrap();
        assert_eq!(
            read_timeline(&timeline_path(&mkv)).unwrap()["validated"],
            true
        );
        std::fs::write(
            timeline_path(&raw),
            serde_json::to_vec(&timeline_fixture(29, 100)).unwrap(),
        )
        .unwrap();
        let rejected = root.join("uncertified.mp4");
        std::fs::copy(&paths[0], &rejected).unwrap();
        assert!(publish_timeline(
            &ffmpeg,
            &raw,
            &rejected,
            &mut Control::new(&cancelled, &mut log)
        )
        .is_err());
        assert_eq!(
            read_timeline(&timeline_path(&rejected)).unwrap()["validated"],
            false
        );
        assert!(raw.is_file());
        std::fs::write(&paths[0], b"changed source").unwrap();
        assert!(concat_clips(
            &ffmpeg,
            &paths,
            &root.join("changed-merge.mp4"),
            &mut Control::new(&cancelled, &mut log)
        )
        .is_err());
    }

    #[test]
    fn capture_gate_rejects_clamped_resolution_fps_and_short_files() {
        let valid = serde_json::json!({"width":3840,"height":2160,"avg_frame_rate":"60/1","duration":"10.016667"});
        assert!(validate_capture_stream(&valid, 3840, 2160, 60, 10.0, 64.0).is_ok());
        assert!(validate_capture_stream(&valid, 1280, 720, 15, 10.0, 64.0).is_err());
        let clamped = serde_json::json!({"width":1920,"height":1080,"avg_frame_rate":"60/1","duration":"10.016667"});
        let error = validate_capture_stream(&clamped, 3840, 2160, 60, 10.0, 64.0)
            .unwrap_err()
            .to_string();
        assert!(error.contains("1920×1080"));
        let wrong_fps = serde_json::json!({"width":3840,"height":2160,"avg_frame_rate":"30/1","duration":"10.0"});
        assert!(validate_capture_stream(&wrong_fps, 3840, 2160, 60, 10.0, 64.0).is_err());
        let short = serde_json::json!({"width":3840,"height":2160,"avg_frame_rate":"60/1","duration":"4.0"});
        let short_error = validate_capture_stream(&short, 3840, 2160, 60, 10.0, 64.0)
            .unwrap_err()
            .to_string();
        assert!(
            short_error.contains("4.000 seconds")
                && short_error.contains("requested 10.000 seconds")
        );
        let four_frames = serde_json::json!({"width":3840,"height":2160,"avg_frame_rate":"60/1","duration":"0.066667"});
        let actual_error = validate_capture_stream(&four_frames, 3840, 2160, 60, 10.234375, 64.0)
            .unwrap_err()
            .to_string();
        assert!(actual_error.contains("0.067 seconds") && actual_error.contains("10.234 seconds"));
    }

    #[test]
    fn capture_gate_missing_duration_uses_decoded_video_count_and_verified_fps() {
        let mut stream = serde_json::json!({"width":320,"height":180,"avg_frame_rate":"30/1","nb_read_frames":"30"});
        assert!(validate_capture_stream(&stream, 320, 180, 30, 1.0, 64.0).is_ok());
        assert!(validate_capture_stream(&stream, 320, 180, 30, 4.0, 64.0).is_err());
        stream["nb_read_frames"] = json_value("2");
        assert!(validate_capture_stream(&stream, 320, 180, 30, 1.0, 64.0).is_err());
        stream["nb_read_frames"] = json_value("N/A");
        assert!(validate_capture_stream(&stream, 320, 180, 30, 1.0, 64.0).is_err());
        stream["nb_read_frames"] = json_value("0");
        assert!(validate_capture_stream(&stream, 320, 180, 30, 1.0, 64.0).is_err());
        stream["nb_read_frames"] = json_value("30");
        stream["avg_frame_rate"] = json_value("0/0");
        assert!(validate_capture_stream(&stream, 320, 180, 30, 1.0, 64.0).is_err());
        stream["avg_frame_rate"] = json_value("15/1");
        assert!(validate_capture_stream(&stream, 320, 180, 30, 1.0, 64.0).is_err());
    }

    #[test]
    #[ignore = "requires bundled CPU FFmpeg via CLIPFORGE_TEST_FFMPEG"]
    fn actual_mkv_native_capture_gate_counts_video_and_rejects_truncation() {
        let ffmpeg = PathBuf::from(
            std::env::var_os("CLIPFORGE_TEST_FFMPEG").expect("set CLIPFORGE_TEST_FFMPEG"),
        );
        let root = tempfile::tempdir().unwrap();
        let mp4 = root.path().join("source.mp4");
        let mkv = root.path().join("long-audio.mkv");
        let short = root.path().join("truncated.mkv");
        let short_mp4 = root.path().join("truncated-source.mp4");
        let path = |file: &Path| file.to_string_lossy().into_owned();
        run(
            &ffmpeg,
            &[
                s("-y"),
                s("-f"),
                s("lavfi"),
                s("-i"),
                s("testsrc2=size=320x180:rate=30"),
                s("-frames:v"),
                s("30"),
                s("-c:v"),
                s("libx264"),
                s("-preset"),
                s("ultrafast"),
                s("-threads"),
                s("1"),
                s("-an"),
                path(&mp4),
            ],
        )
        .unwrap();
        run(
            &ffmpeg,
            &[
                s("-y"),
                s("-i"),
                path(&mp4),
                s("-f"),
                s("lavfi"),
                s("-i"),
                s("sine=frequency=600:sample_rate=48000:duration=4"),
                s("-map"),
                s("0:v:0"),
                s("-map"),
                s("1:a:0"),
                s("-c:v"),
                s("copy"),
                s("-c:a"),
                s("pcm_s16le"),
                s("-threads"),
                s("1"),
                path(&mkv),
            ],
        )
        .unwrap();
        run(
            &ffmpeg,
            &[
                s("-y"),
                s("-i"),
                path(&mp4),
                s("-map"),
                s("0:v:0"),
                s("-frames:v"),
                s("2"),
                s("-c:v"),
                s("copy"),
                s("-an"),
                path(&short_mp4),
            ],
        )
        .unwrap();
        // Some Matroska files infer stream.duration from this longer audio
        // track. The negative must reject using its two decoded video frames.
        run(
            &ffmpeg,
            &[
                s("-y"),
                s("-i"),
                path(&short_mp4),
                s("-f"),
                s("lavfi"),
                s("-i"),
                s("sine=frequency=600:sample_rate=48000:duration=4"),
                s("-map"),
                s("0:v:0"),
                s("-map"),
                s("1:a:0"),
                s("-c:v"),
                s("copy"),
                s("-c:a"),
                s("pcm_s16le"),
                s("-threads"),
                s("1"),
                path(&short),
            ],
        )
        .unwrap();
        let cancelled = AtomicBool::new(false);
        let mut log = |_: String| {};
        let mut control = Control::new(&cancelled, &mut log);
        for file in [&mkv, &short] {
            let mut metadata = Command::new(ffprobe_exe(&ffmpeg));
            metadata
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=duration:format=duration",
                    "-of",
                    "json",
                ])
                .arg(file);
            let output =
                monitored_output(&mut metadata, 1.0, &mut |_| {}, &mut control, None).unwrap();
            let info: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            if file == &mkv {
                assert!(info["streams"][0].get("duration").is_none());
            }
            assert!(
                info["format"]["duration"]
                    .as_str()
                    .unwrap()
                    .parse::<f64>()
                    .unwrap()
                    > 3.9
            );
        }
        validate_native_capture(&ffmpeg, &mp4, 320, 180, 30, 1.0, 64.0, &mut control).unwrap();
        validate_native_capture(&ffmpeg, &mkv, 320, 180, 30, 1.0, 64.0, &mut control).unwrap();
        assert!(
            validate_native_capture(&ffmpeg, &mkv, 320, 180, 30, 4.0, 64.0, &mut control).is_err()
        );
        let error = validate_native_capture(&ffmpeg, &short, 320, 180, 30, 1.0, 64.0, &mut control)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("0.067 seconds") && error.contains("requested 1.000 seconds"),
            "{error}"
        );
        if let Some(actual) = std::env::var_os("CLIPFORGE_TEST_MKV_CAPTURE") {
            validate_native_capture(
                &ffmpeg,
                &PathBuf::from(actual),
                1920,
                1080,
                60,
                18.3,
                64.0,
                &mut control,
            )
            .unwrap();
            println!("Actual parent-owned 1080p60 MKV capture gate passed; original source was read only.");
        }
    }

    #[test]
    fn progress_fixture() {
        use std::io::Write;
        let Ok(mode) = std::env::var("DEMODESK_ENCODE_TEST_MODE") else {
            return;
        };
        if mode == "pts-live" || mode == "pts-repeat" {
            for frame in 0..8 {
                std::thread::sleep(Duration::from_millis(400));
                let time = if mode == "pts-live" {
                    frame as f64 / 30.0
                } else {
                    0.0
                };
                println!("        {{\n            \"best_effort_timestamp_time\": \"{time:.6}\"\n        }},");
                if mode == "pts-repeat" {
                    println!("frame={}\nnoise traffic", frame + 1);
                }
                std::io::stdout().flush().unwrap();
            }
            return;
        }
        if mode == "count-live" {
            for frame in 1..=8 {
                std::thread::sleep(Duration::from_millis(400));
                println!("frame={frame}\nprogress=continue");
                std::io::stdout().flush().unwrap();
            }
            println!("progress=end");
            return;
        }
        if mode == "count-quiet" {
            std::thread::sleep(Duration::from_secs(3));
            println!("frame=8\nprogress=end");
            return;
        }
        if mode == "stuck" || mode == "quiet" {
            std::thread::sleep(Duration::from_secs(if mode == "stuck" { 4 } else { 2 }));
            return;
        }
        if mode == "cpu" {
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(2) {
                std::hint::black_box((0..1000).sum::<u64>());
            }
            return;
        }
        if mode == "bursty" {
            for frame in 1..=4 {
                std::thread::sleep(Duration::from_millis(750));
                println!("frame={frame}");
                std::io::stdout().flush().unwrap();
            }
            return;
        }
        for frame in 1..=40 {
            let value = if matches!(mode.as_str(), "size" | "heartbeat") {
                1
            } else {
                frame
            };
            let bytes = if mode == "size" { frame * 64 } else { 0 };
            println!(
                "frame={value}\nout_time_us={}\ntotal_size={bytes}\nprogress=continue",
                frame * 1000
            );
            std::io::stdout().flush().unwrap();
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn progress_command(mode: &str) -> Command {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "render::encode::tests::progress_fixture",
            "--nocapture",
        ])
        .env("DEMODESK_ENCODE_TEST_MODE", mode);
        cmd
    }

    #[test]
    fn decoded_video_count_progress_outlives_quiet_window_and_requires_completion() {
        let cancelled = AtomicBool::new(false);
        let mut log = |_: String| {};
        let mut control = Control::new(&cancelled, &mut log);
        control.check_interval = Duration::from_millis(250);
        let started = Instant::now();
        assert_eq!(
            decoded_video_count_output(&mut progress_command("count-live"), 1.0, &mut control)
                .unwrap(),
            8
        );
        assert!(started.elapsed() > Duration::from_secs(3));
        let quiet =
            decoded_video_count_output(&mut progress_command("count-quiet"), 1.0, &mut control)
                .unwrap_err();
        assert!(stalled(&quiet));
        let incomplete =
            decoded_video_count_output(&mut progress_command("frames"), 1.0, &mut control)
                .unwrap_err();
        assert!(incomplete.to_string().contains("did not report completion"));
        let already_cancelled = AtomicBool::new(true);
        assert_eq!(
            decoded_video_count_output(
                &mut Command::new("must-not-start"),
                1.0,
                &mut Control::new(&already_cancelled, &mut log)
            )
            .unwrap_err()
            .to_string(),
            "cancelled"
        );
    }

    #[test]
    fn decoded_timestamp_progress_requires_complete_finite_increasing_json_values() {
        let mut probe = DecodedTimestampProgress::default();
        assert!(!probe.observe(b"  \"best_effort_time").unwrap());
        assert!(!probe.observe(b"stamp_time\": \"0.000000\"").unwrap());
        assert!(probe.observe(b",\n").unwrap());
        for bytes in [
            b"\"best_effort_timestamp_time\": \"0\"\n".as_slice(),
            b"\"best_effort_timestamp_time\": \"-1\"\n",
            b"\"best_effort_timestamp_time\": \"NaN\"\n",
            b"\"best_effort_timestamp_time\": \"inf\"\n",
            b"\"best_effort_timestamp_time\": 1\n",
            b"\"best_effort_timestamp_time\": \"bad\"\n",
            b"\"best_effort_timestamp_time\": \"0.5\",,\n",
            b"frame=100\n",
            b"\"note\": \"best_effort_timestamp_time\"\n",
        ] {
            assert!(!probe.observe(bytes).unwrap());
        }
        assert!(probe
            .observe(b"\"best_effort_timestamp_time\": \"0.033333\"\n")
            .unwrap());
        assert!(!probe
            .observe(b"\"best_effort_timestamp_time\": \"0.020000\"\n")
            .unwrap());
        assert_eq!(probe.last_pts, Some(0.033333));
        probe.bytes = 128 * 1024 * 1024;
        assert_eq!(
            probe.observe(b"x").unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn decoded_timestamp_probe_progress_prevents_false_stall_and_keeps_cancel() {
        let cancel = AtomicBool::new(false);
        let mut log = |_: String| {};
        let mut control = Control::new(&cancel, &mut log);
        control.check_interval = Duration::from_millis(250);
        let started = Instant::now();
        assert!(
            monitored_frame_probe_output(&mut progress_command("pts-live"), &mut control)
                .unwrap()
                .status
                .success()
        );
        assert!(started.elapsed() > Duration::from_secs(3));
        assert!(stalled(
            &monitored_frame_probe_output(&mut progress_command("pts-repeat"), &mut control)
                .unwrap_err()
        ));
        assert!(stalled(
            &monitored_frame_probe_output(&mut progress_command("count-quiet"), &mut control)
                .unwrap_err()
        ));
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(900));
                cancel.store(true, Ordering::Relaxed);
            });
            let started = Instant::now();
            let error =
                monitored_frame_probe_output(&mut progress_command("pts-live"), &mut control)
                    .unwrap_err();
            assert_eq!(error.to_string(), "cancelled");
            assert!(started.elapsed() < Duration::from_secs(2));
        });
    }

    #[test]
    fn stalled_encoding_retries_three_times_but_advancing_encoding_finishes() {
        let cancel = AtomicBool::new(false);
        let mut logs = Vec::new();
        let mut log = |line| logs.push(line);
        let mut control = Control::new(&cancel, &mut log);
        control.check_interval = Duration::from_millis(250);
        let mut attempts = 0;
        let error = retry_stalled(&mut control, |control| {
            attempts += 1;
            let started = Instant::now();
            let result = monitored_output(
                &mut progress_command(if attempts == 4 { "heartbeat" } else { "stuck" }),
                1.0,
                &mut |_| {},
                control,
                None,
            );
            assert!(started.elapsed() >= control.check_interval * 6);
            result
        })
        .unwrap_err();
        assert!(stalled(&error));
        assert_eq!(attempts, 4);
        for mode in ["advancing", "size", "bursty"] {
            let started = Instant::now();
            let out = monitored_output(
                &mut progress_command(mode),
                1.0,
                &mut |_| {},
                &mut control,
                None,
            )
            .unwrap();
            assert!(out.status.success());
            assert!(started.elapsed() > control.check_interval);
        }
        assert_eq!(
            logs.iter().filter(|line| line.contains("retry ")).count(),
            3
        );
    }

    #[test]
    fn cancelled_encoding_stops_promptly_without_retry_or_nvenc_fallback() {
        let cancel = AtomicBool::new(false);
        let mut log = |_| {};
        let mut control = Control::new(&cancel, &mut log);
        let mut attempts = 0;
        let mut compatible = false;
        let mut cancelled_at = None;
        let error = retry_stalled(&mut control, |control| {
            with_nvenc_fallback(&mut compatible, |_| {
                attempts += 1;
                monitored_output(
                    &mut progress_command("advancing"),
                    1.0,
                    &mut |_| {
                        cancelled_at.get_or_insert_with(Instant::now);
                        cancel.store(true, Ordering::Relaxed);
                    },
                    control,
                    None,
                )
            })
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "cancelled");
        assert_eq!(attempts, 1);
        assert!(!compatible);
        assert!(cancelled_at.unwrap().elapsed() < Duration::from_secs(1));
        // Already-cancelled work must never launch a child.
        assert_eq!(
            monitored_output(
                &mut Command::new("missing-encoder"),
                1.0,
                &mut |_| {},
                &mut control,
                None,
            )
            .unwrap_err()
            .to_string(),
            "cancelled"
        );
    }

    #[test]
    fn retries_stop_on_success_or_non_stall_errors() {
        let cancel = AtomicBool::new(false);
        let mut log = |_| {};
        let mut control = Control::new(&cancel, &mut log);
        let mut attempts = 0;
        let value = retry_stalled(&mut control, |_| {
            attempts += 1;
            if attempts < 3 {
                Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "stuck").into())
            } else {
                Ok(42)
            }
        })
        .unwrap();
        assert_eq!((value, attempts), (42, 3));
        attempts = 0;
        assert!(retry_stalled::<()>(&mut control, |_| {
            attempts += 1;
            Err(anyhow!("invalid codec"))
        })
        .is_err());
        assert_eq!(attempts, 1);
    }

    #[test]
    fn output_file_changes_keep_silent_process_alive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("encode.mp4");
        let cancel = AtomicBool::new(false);
        let mut log = |_| {};
        let mut control = Control::new(&cancel, &mut log);
        control.check_interval = Duration::from_millis(250);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                use std::io::Write;
                let mut file = std::fs::File::create(&path).unwrap();
                for _ in 0..50 {
                    file.write_all(&[0; 64]).unwrap();
                    file.flush().unwrap();
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
            let output = monitored_output(
                &mut progress_command("quiet"),
                1.0,
                &mut |_| {},
                &mut control,
                Some(&path),
            )
            .unwrap();
            assert!(output.status.success());
        });
    }

    #[test]
    fn cpu_activity_without_media_progress_is_stalled() {
        let cancel = AtomicBool::new(false);
        let mut log = |_| {};
        let mut control = Control::new(&cancel, &mut log);
        control.check_interval = Duration::from_millis(250);
        let output = monitored_output(
            &mut progress_command("cpu"),
            1.0,
            &mut |_| {},
            &mut control,
            None,
        )
        .unwrap_err();
        assert!(stalled(&output));
    }

    #[test]
    fn compatibility_retry_is_bounded_and_reused() {
        let mut compatible = false;
        let mut calls = vec![];
        let result = with_nvenc_fallback(&mut compatible, |fallback| {
            calls.push(fallback);
            if fallback {
                Ok(42)
            } else {
                Err(anyhow!("unsupported B-frames"))
            }
        })
        .unwrap();
        assert_eq!(result, 42);
        assert_eq!(calls, [false, true]);
        calls.clear();
        assert!(with_nvenc_fallback::<()>(&mut compatible, |fallback| {
            calls.push(fallback);
            Err(anyhow!("still fails"))
        })
        .is_err());
        assert_eq!(calls, [true]);
        calls.clear();
        let error = with_nvenc_fallback::<()>(&mut false, |fallback| {
            calls.push(fallback);
            Err(anyhow!(if fallback {
                "fallback error"
            } else {
                "primary error"
            }))
        })
        .unwrap_err()
        .to_string();
        assert_eq!(calls, [false, true]);
        assert!(error.contains("primary error") && error.contains("fallback error"));
        calls.clear();
        with_nvenc_fallback(&mut false, |fallback| {
            calls.push(fallback);
            Ok(())
        })
        .unwrap();
        assert_eq!(calls, [false]);
    }

    #[test]
    fn size_budget_accounts_for_decimal_units_audio_and_invalid_inputs() {
        assert_eq!(video_bitrate_for_size(20.0, 60.0, 192), 2421);
        assert_eq!(video_bitrate_for_size(20.0, 60.0, 0), 2613);
        assert_eq!(video_bitrate_for_size(0.1, 60.0, 192), 0);
        for seconds in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(video_bitrate_for_size(20.0, seconds, 192), 0);
        }
    }

    #[test]
    fn record_presets_preserve_encoder_and_quality_without_resizing() {
        for (codec, _) in CODECS {
            let preset = record_preset(codec, 20, 90);
            assert!(preset.contains(&format!("-c:v {codec}")));
            assert!(preset.contains(if codec.contains("nvenc") {
                "-rc constqp -qp 20"
            } else if codec.contains("_amf") {
                "-rc cqp -qp_i 20"
            } else {
                "-crf 20"
            }));
            assert!(!preset.contains("scale="));
            assert!(!preset.contains("-r "));
            if codec.contains("nvenc") {
                assert!(preset.contains(NVENC_RECORDING));
                assert!(preset.contains("-g 180"));
                assert!(!preset.contains("-cq "));
                assert!(record_preset(codec, 20, 60).contains("-g 120"));
            } else if codec.contains("_amf") {
                assert!(preset.contains("-quality quality"));
                assert!(preset.contains("-g 180"));
            } else {
                assert!(preset.contains(if is_hevc(codec) {
                    "-preset fast -profile:v main"
                } else {
                    "-preset veryfast -profile:v high"
                }));
                assert!(preset.contains("-g 180"));
                assert!(!preset.contains("-tune"));
            }
        }
    }
    #[test]
    #[ignore = "requires FFMPEG; set TEST_NVENC=1 to include NVIDIA encoders"]
    fn real_encoding_preserves_format_and_never_publishes_oversized_files() {
        let ffmpeg =
            PathBuf::from(std::env::var("FFMPEG").expect("set FFMPEG to ffmpeg executable"));
        let started = std::time::Instant::now();
        let mut live_update = false;
        let sink = if cfg!(windows) { "NUL" } else { "/dev/null" };
        run_progress(
            &ffmpeg,
            &[
                s("-re"),
                s("-f"),
                s("lavfi"),
                s("-i"),
                s("testsrc2=size=64x64:rate=30"),
                s("-t"),
                s("3"),
                s("-f"),
                s("null"),
                s(sink),
            ],
            3.0,
            &mut |p| {
                if p > 0.0 && p < 0.5 && started.elapsed().as_secs_f64() < 2.5 {
                    live_update = true;
                }
            },
            &mut Control::new(&AtomicBool::new(false), &mut |_| {}),
        )
        .unwrap();
        assert!(
            live_update,
            "FFmpeg progress must arrive before the process exits"
        );
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("source.mkv");
        let mut generate = [
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=960x540:rate=90",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=900:sample_rate=48000",
            "-t",
            "4",
            "-c:v",
            "ffv1",
            "-c:a",
            "pcm_s16le",
        ]
        .map(s)
        .to_vec();
        generate.push(input.to_string_lossy().into_owned());
        run(&ffmpeg, &generate).unwrap();
        let cancelled_output = dir.path().join("cancelled.mp4");
        std::fs::write(&cancelled_output, b"existing output").unwrap();
        let cancel = AtomicBool::new(false);
        let mut logs = Vec::new();
        let source_bytes = std::fs::metadata(&input).unwrap().len();
        let error = encode_to_size_with_progress(
            &ffmpeg,
            &input,
            &cancelled_output,
            SizeOptions {
                max_size_mb: 0.5,
                codec: "libx264",
                audio_kbps: 192,
            },
            &mut false,
            &mut |p| {
                if p > 0.0 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
            &mut Control::new(&cancel, &mut |line| logs.push(line)),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "cancelled");
        assert_eq!(std::fs::metadata(&input).unwrap().len(), source_bytes);
        assert_eq!(
            std::fs::read(&cancelled_output).unwrap(),
            b"existing output"
        );
        assert!(!logs.iter().any(|line| line.contains("retry ")));
        assert!(!std::fs::read_dir(dir.path()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("encode-")));
        for (codec, _) in CODECS {
            if (codec.contains("nvenc") && std::env::var_os("TEST_NVENC").is_none())
                || (codec.contains("_amf") && std::env::var_os("TEST_AMF").is_none())
            {
                continue;
            }
            let output = dir.path().join(format!("{codec}.mp4"));
            let mut updates = vec![];
            let capture = dir.path().join(format!("capture-{codec}.mp4"));
            let mut args = vec![s("-y"), s("-i"), input.to_string_lossy().into_owned()];
            let (preset, _) = checked_record_preset(
                &ffmpeg,
                &super::super::RenderOptions {
                    codec: codec.into(),
                    crf: 20,
                    fps: 90,
                    width: 960,
                    height: 540,
                    ..super::super::RenderOptions::default()
                },
                &mut |_| {},
                &AtomicBool::new(false),
            )
            .unwrap();
            args.extend(preset.split_whitespace().map(s));
            args.extend([s("-c:a"), s("aac"), capture.to_string_lossy().into_owned()]);
            run(&ffmpeg, &args).unwrap();
            let result = encode_to_size_with_progress(
                &ffmpeg,
                &capture,
                &output,
                SizeOptions {
                    max_size_mb: 0.5,
                    codec: codec,
                    audio_kbps: 192,
                },
                &mut false,
                &mut |p| updates.push(p),
                &mut Control::new(&AtomicBool::new(false), &mut |_| {}),
            )
            .unwrap();
            assert_eq!(updates.last(), Some(&1.0));
            // Fast hardware encodes may first report at pass completion (0.9), before verification.
            assert!(updates.iter().any(|p| *p > 0.0 && *p < 1.0));
            assert!(
                updates.windows(2).all(|w| w[0] <= w[1]),
                "progress must not reset between passes or retries: {updates:?}"
            );
            assert!(result.bytes > 0 && result.bytes <= 500_000);
            let mut probe = Command::new(ffprobe_exe(&ffmpeg));
            probe
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "stream=codec_type,width,height,avg_frame_rate,sample_rate,channels",
                    "-of",
                    "json",
                ])
                .arg(&output);
            let out = ProcessTree::new().unwrap().output(&mut probe).unwrap();
            let info: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            let streams = info["streams"].as_array().unwrap();
            let video = streams.iter().find(|v| v["codec_type"] == "video").unwrap();
            assert_eq!(
                (video["width"].as_u64(), video["height"].as_u64()),
                (Some(960), Some(540))
            );
            assert_eq!(video["avg_frame_rate"], "90/1");
            let audio = streams.iter().find(|v| v["codec_type"] == "audio").unwrap();
            assert_eq!(audio["sample_rate"], "48000");
            assert_eq!(audio["channels"], 2);
            let saved = std::fs::read(&output).unwrap();
            assert!(encode_to_size(&ffmpeg, &input, &output, 0.001, codec, 192).is_err());
            assert_eq!(std::fs::read(&output).unwrap(), saved);
            if codec.contains("nvenc") {
                let compatible_capture = dir.path().join(format!("compatible-{codec}.mp4"));
                let compatible_preset = record_preset(codec, 20, 90)
                    .replace(NVENC_RECORDING, NVENC_COMPATIBLE)
                    .replace("main -b_ref_mode middle", "main");
                let mut args = vec![s("-y"), s("-i"), input.to_string_lossy().into_owned()];
                args.extend(compatible_preset.split_whitespace().map(s));
                args.extend([
                    s("-c:a"),
                    s("aac"),
                    compatible_capture.to_string_lossy().into_owned(),
                ]);
                run(&ffmpeg, &args).unwrap();
                let fitted = dir.path().join(format!("compatible-fitted-{codec}.mp4"));
                let result = encode_to_size_with_progress(
                    &ffmpeg,
                    &compatible_capture,
                    &fitted,
                    SizeOptions {
                        max_size_mb: 0.5,
                        codec: codec,
                        audio_kbps: 192,
                    },
                    &mut true,
                    &mut |_| {},
                    &mut Control::new(&AtomicBool::new(false), &mut |_| {}),
                )
                .unwrap();
                assert!(result.bytes > 0 && result.bytes <= 500_000);
                assert!((probe_duration_seconds(&ffmpeg, &fitted).unwrap() - 4.0).abs() < 0.1);
            }
            println!(
                "{codec}: {} bytes, 960x540 at 90 FPS, stereo 48 kHz; failure preserves output",
                result.bytes
            );
        }
        let vfr = dir.path().join("vfr.mkv");
        run(
            &ffmpeg,
            &[
                s("-y"),
                s("-i"),
                input.to_string_lossy().into_owned(),
                s("-vf"),
                s("select=mod(n\\,5)"),
                s("-fps_mode"),
                s("passthrough"),
                s("-c:v"),
                s("ffv1"),
                s("-an"),
                vfr.to_string_lossy().into_owned(),
            ],
        )
        .unwrap();
        let fitted = dir.path().join("vfr.mp4");
        encode_to_size(&ffmpeg, &vfr, &fitted, 0.5, "libx264", 192).unwrap();
        let timestamps = |path: &Path| -> Vec<f64> {
            let out = Command::new(ffprobe_exe(&ffmpeg))
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "frame=best_effort_timestamp_time",
                    "-of",
                    "json",
                ])
                .arg(path)
                .output()
                .unwrap();
            assert!(out.status.success());
            let info: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            info["frames"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| {
                    f["best_effort_timestamp_time"]
                        .as_str()
                        .unwrap()
                        .parse()
                        .unwrap()
                })
                .collect()
        };
        let before = timestamps(&vfr);
        let after = timestamps(&fitted);
        assert_eq!(
            before.len(),
            after.len(),
            "Size fitting must not duplicate or drop VFR frames"
        );
        assert!(
            before
                .iter()
                .zip(&after)
                .all(|(a, b)| ((a - before[0]) - (b - after[0])).abs() < 0.002),
            "Size fitting must preserve frame timing"
        );
        let silent = dir.path().join("silent.mkv");
        run(
            &ffmpeg,
            &[
                s("-y"),
                s("-i"),
                input.to_string_lossy().into_owned(),
                s("-an"),
                s("-c:v"),
                s("copy"),
                silent.to_string_lossy().into_owned(),
            ],
        )
        .unwrap();
        let output = dir.path().join("silent.mp4");
        assert!(
            encode_to_size(&ffmpeg, &silent, &output, 0.5, "libx264", 192)
                .unwrap()
                .bytes
                <= 500_000
        );
        assert!(
            !probe_media(
                &ffmpeg,
                &output,
                &mut Control::new(&AtomicBool::new(false), &mut |_| {})
            )
            .unwrap()
            .1
        );
    }
}
