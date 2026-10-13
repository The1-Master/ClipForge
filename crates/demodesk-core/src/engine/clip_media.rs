//! Lazy, serialized CPU-only gallery measurements. Inputs are registered outputs, never raw UI paths.
use crate::{
    render::{encode::ffprobe_exe, process::ProcessTree},
    store::Store,
};
use anyhow::{anyhow, ensure, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipMedia {
    pub output_key: String,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration_seconds: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster_path: Option<String>,
}

#[derive(Default)]
pub(super) struct MediaWorker {
    serial: Mutex<()>,
    pending: Mutex<HashMap<String, Vec<Arc<AtomicBool>>>>,
    version: Mutex<Option<(String, String)>>,
}

fn key(job: &str, output: &str) -> String {
    format!("{job}\0{output}")
}
impl MediaWorker {
    pub fn while_idle<T>(&self, action: impl FnOnce() -> T) -> T {
        let _serial = self.serial.lock().unwrap();
        action()
    }
    pub fn cancel(&self, job: &str, output: &str) {
        if let Some(flags) = self.pending.lock().unwrap().get(&key(job, output)) {
            for flag in flags {
                flag.store(true, Ordering::SeqCst);
            }
        }
    }
    pub fn cancel_job(&self, job: &str) {
        for (key, flags) in self.pending.lock().unwrap().iter() {
            if key.starts_with(&format!("{job}\0")) {
                for flag in flags {
                    flag.store(true, Ordering::SeqCst);
                }
            }
        }
    }
    pub fn cancel_all(&self) {
        for flags in self.pending.lock().unwrap().values() {
            for flag in flags {
                flag.store(true, Ordering::SeqCst);
            }
        }
    }
    pub fn get(
        &self,
        store: &Store,
        root: &Path,
        job_id: &str,
        output_key: &str,
        ffmpeg: Option<&Path>,
        shutdown: &AtomicBool,
    ) -> Result<ClipMedia> {
        ensure!(valid_job_id(job_id), "Invalid export job.");
        let request_key = key(job_id, output_key);
        let cancel = Arc::new(AtomicBool::new(false));
        self.pending
            .lock()
            .unwrap()
            .entry(request_key.clone())
            .or_default()
            .push(cancel.clone());
        let result = (|| {
            let _serial = loop {
                check(&cancel, shutdown)?;
                if let Ok(lock) = self.serial.try_lock() {
                    break lock;
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            let job = store
                .get_job(job_id)
                .ok_or_else(|| anyhow!("This export job is missing."))?;
            let output = job
                .outputs
                .iter()
                .find(|o| o.file == output_key)
                .ok_or_else(|| {
                    anyhow!("This video is not a registered output of the selected export.")
                })?;
            let job_dir = contained(root, &store.job_dir(job_id))?;
            let video = contained(&job_dir, Path::new(&output.file))
                .map_err(|_| anyhow!("This video is missing or is outside its export folder."))?;
            ensure!(video.is_file(), "This video is missing.");
            let ffmpeg = ffmpeg.ok_or_else(|| {
                anyhow!("Install or select FFmpeg in Tools to load video details.")
            })?;
            ensure!(
                ffmpeg.is_file() && ffprobe_exe(ffmpeg).is_file(),
                "FFmpeg or FFprobe is missing. Check Tools in Settings."
            );
            let tool_stamp = stamp(ffmpeg)?;
            let version_key = format!("{}:{tool_stamp}", ffmpeg.display());
            let version = {
                let cached = self.version.lock().unwrap().clone();
                if let Some((old, value)) = cached.filter(|(old, _)| old == &version_key) {
                    let _ = old;
                    value
                } else {
                    let mut command = Command::new(ffmpeg);
                    command.arg("-version");
                    let bytes = run(&mut command, &cancel, shutdown, Duration::from_secs(10))?;
                    let value = String::from_utf8_lossy(&bytes)
                        .lines()
                        .next()
                        .unwrap_or("")
                        .to_string();
                    ensure!(!value.is_empty(), "FFmpeg did not report its version.");
                    *self.version.lock().unwrap() = Some((version_key.clone(), value.clone()));
                    value
                }
            };
            let video_stamp = stamp(&video)?;
            let probe_stamp = stamp(&ffprobe_exe(ffmpeg))?;
            let cache_key = format!(
                "{:x}",
                Sha256::digest(format!(
                    "media-v1:{output_key}:{video_stamp}:{version_key}:{version}:{probe_stamp}"
                ))
            );
            let media_dir = job_dir.join(".media");
            if media_dir.exists() {
                contained(&job_dir, &media_dir)?;
            }
            fs::create_dir_all(&media_dir)?;
            let media_dir = contained(&job_dir, &media_dir)?;
            let metadata_path = media_dir.join(format!("{cache_key}.json"));
            if metadata_path.is_file() {
                contained(&media_dir, &metadata_path)?;
                if let Ok(value) = serde_json::from_slice::<ClipMedia>(&fs::read(&metadata_path)?) {
                    if value.output_key == output_key
                        && value
                            .poster_path
                            .as_ref()
                            .is_none_or(|p| contained(&media_dir, Path::new(p)).is_ok())
                    {
                        check(&cancel, shutdown)?;
                        return Ok(value);
                    }
                }
            }
            let mut probe = Command::new(ffprobe_exe(ffmpeg));
            probe
                .args([
                    "-v",
                    "error",
                    "-protocol_whitelist",
                    "file",
                    "-format_whitelist",
                    "mov,matroska",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=width,height,avg_frame_rate,duration:format=duration",
                    "-of",
                    "json",
                ])
                .arg(&video);
            let bytes = run(&mut probe, &cancel, shutdown, Duration::from_secs(20))?;
            let mut value = measurements(&serde_json::from_slice(&bytes)?, output_key)?;
            let poster = media_dir.join(format!("{cache_key}.jpg"));
            let temp = tempfile::Builder::new()
                .prefix("poster-")
                .suffix(".jpg")
                .tempfile_in(&media_dir)?;
            let mut command = Command::new(ffmpeg);
            command
                .args(["-v", "error", "-nostdin", "-y", "-threads", "1", "-ss"])
                .arg(format!("{:.3}", (value.duration_seconds * 0.25).min(4.0)))
                .args([
                    "-protocol_whitelist",
                    "file",
                    "-format_whitelist",
                    "mov,matroska",
                ])
                .arg("-i")
                .arg(&video)
                .args([
                    "-map",
                    "0:v:0",
                    "-frames:v",
                    "1",
                    "-vf",
                    "scale=640:-2",
                    "-threads",
                    "1",
                    "-q:v",
                    "3",
                    "-f",
                    "image2",
                ])
                .arg(temp.path());
            if run(&mut command, &cancel, shutdown, Duration::from_secs(30)).is_ok()
                && fs::metadata(temp.path()).is_ok_and(|m| m.len() > 0)
            {
                temp.persist(&poster).map_err(|e| e.error)?;
                value.poster_path = Some(
                    contained(&media_dir, &poster)?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            check(&cancel, shutdown)?;
            ensure!(
                stamp(&video)? == video_stamp,
                "This video changed while its details were loading. Try again."
            );
            let mut temp = tempfile::NamedTempFile::new_in(&media_dir)?;
            use std::io::Write;
            temp.write_all(&serde_json::to_vec(&value)?)?;
            temp.as_file().sync_all()?;
            temp.persist(&metadata_path).map_err(|e| e.error)?;
            Ok(value)
        })();
        let mut pending = self.pending.lock().unwrap();
        if let Some(flags) = pending.get_mut(&request_key) {
            flags.retain(|flag| !Arc::ptr_eq(flag, &cancel));
            if flags.is_empty() {
                pending.remove(&request_key);
            }
        }
        result
    }
}

fn valid_job_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|v| v.is_ascii_alphanumeric() || v == b'-' || v == b'_')
}
fn contained(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(root)?;
    let actual = fs::canonicalize(path)?;
    ensure!(
        actual.starts_with(&root) && actual != root,
        "Output is outside its owning export folder."
    );
    Ok(actual)
}
fn stamp(path: &Path) -> Result<String> {
    let m = fs::metadata(path)?;
    Ok(format!(
        "{}:{}",
        m.len(),
        m.modified()?.duration_since(UNIX_EPOCH)?.as_nanos()
    ))
}
fn check(cancel: &AtomicBool, shutdown: &AtomicBool) -> Result<()> {
    ensure!(
        !cancel.load(Ordering::SeqCst) && !shutdown.load(Ordering::SeqCst),
        "Video details loading was cancelled."
    );
    Ok(())
}
fn run(
    command: &mut Command,
    cancel: &AtomicBool,
    shutdown: &AtomicBool,
    timeout: Duration,
) -> Result<Vec<u8>> {
    check(cancel, shutdown)?;
    let tree = ProcessTree::new()?;
    tree.set_low_priority()?;
    let deadline = Instant::now() + timeout;
    let mut total = 0;
    let output = tree.output_with_progress(command, &mut |bytes| {
        total += bytes.len();
        if cancel.load(Ordering::SeqCst) || shutdown.load(Ordering::SeqCst) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Video details loading was cancelled.",
            ));
        }
        if Instant::now() >= deadline || total > 1024 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Video details loading exceeded its limit.",
            ));
        }
        Ok(())
    })?;
    ensure!(
        output.status.success(),
        "The video could not be measured. Check the file and FFmpeg in Tools."
    );
    Ok(output.stdout)
}
fn measurements(json: &serde_json::Value, output_key: &str) -> Result<ClipMedia> {
    let stream = json["streams"]
        .as_array()
        .and_then(|v| v.first())
        .ok_or_else(|| anyhow!("The file has no readable video track."))?;
    let width = stream["width"]
        .as_u64()
        .filter(|v| *v > 0 && *v <= 16384)
        .ok_or_else(|| anyhow!("Invalid video width."))? as u32;
    let height = stream["height"]
        .as_u64()
        .filter(|v| *v > 0 && *v <= 16384)
        .ok_or_else(|| anyhow!("Invalid video height."))? as u32;
    let rate = stream["avg_frame_rate"].as_str().unwrap_or("");
    let (n, d) = rate
        .split_once('/')
        .ok_or_else(|| anyhow!("Video frame rate is unavailable."))?;
    let fps = n.parse::<f64>()? / d.parse::<f64>()?;
    let duration_seconds = json["format"]["duration"]
        .as_str()
        .or_else(|| stream["duration"].as_str())
        .ok_or_else(|| anyhow!("Video duration is unavailable."))?
        .parse::<f64>()?;
    ensure!(
        fps.is_finite() && fps > 0.0 && duration_seconds.is_finite() && duration_seconds > 0.0,
        "Video timing is invalid."
    );
    Ok(ClipMedia {
        output_key: output_key.into(),
        width,
        height,
        fps,
        duration_seconds,
        poster_path: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn measured_metadata_uses_real_rational_fps_and_rejects_bad_timing() {
        let mut v = serde_json::json!({"streams":[{"width":3840,"height":2160,"avg_frame_rate":"60000/1001"}],"format":{"duration":"8.008"}});
        let media = measurements(&v, "output").unwrap();
        assert_eq!(media.width, 3840);
        assert!((media.fps - 59.94005994).abs() < 0.000001);
        v["streams"][0]["avg_frame_rate"] = serde_json::json!("60/0");
        assert!(measurements(&v, "output").is_err());
    }
    #[test]
    fn output_paths_and_job_identifiers_cannot_escape() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        fs::write(root.path().join("video.mp4"), b"fixture").unwrap();
        fs::write(other.path().join("video.mp4"), b"outside").unwrap();
        assert!(contained(root.path(), &root.path().join("video.mp4")).is_ok());
        assert!(contained(root.path(), &other.path().join("video.mp4")).is_err());
        assert!(!valid_job_id("../outside"));
        assert!(!valid_job_id("C:\\outside"));
        assert!(valid_job_id("20261013T010100-0123"));
    }
    #[test]
    fn pending_media_work_can_be_cancelled_before_a_helper_starts() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path().into()).unwrap();
        let job = store.new_job("demo", vec![], Default::default()).unwrap();
        let worker = Arc::new(MediaWorker::default());
        let guard = worker.serial.lock().unwrap();
        let background = worker.clone();
        let root_path = root.path().to_path_buf();
        let id = job.id.clone();
        let thread = std::thread::spawn(move || {
            background.get(
                &store,
                &root_path,
                &id,
                "registered.mp4",
                None,
                &AtomicBool::new(false),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while worker.pending.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        worker.cancel(&job.id, "registered.mp4");
        drop(guard);
        assert!(thread
            .join()
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("cancelled"));
        assert!(worker.pending.lock().unwrap().is_empty());
    }
    #[test]
    #[ignore = "requires the bundled CPU FFmpeg via CLIPFORGE_TEST_FFMPEG"]
    fn actual_media_cache_is_measured_reused_and_invalidated() {
        use crate::store::JobOutput;
        let ffmpeg = PathBuf::from(std::env::var_os("CLIPFORGE_TEST_FFMPEG").unwrap());
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path().into()).unwrap();
        let mut job = store.new_job("demo", vec![], Default::default()).unwrap();
        let video = store.job_dir(&job.id).join("fixture.mp4");
        let fixture = |duration: &str| {
            let mut command = Command::new(&ffmpeg);
            command
                .args([
                    "-v",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=c=orange:s=320x180:r=20",
                    "-t",
                    duration,
                    "-threads",
                    "1",
                    "-c:v",
                    "libx264",
                ])
                .arg(&video);
            run(
                &mut command,
                &AtomicBool::new(false),
                &AtomicBool::new(false),
                Duration::from_secs(15),
            )
            .unwrap();
        };
        fixture("2");
        job.outputs.push(JobOutput {
            file: video.to_string_lossy().into_owned(),
            bytes: fs::metadata(&video).unwrap().len(),
            highlight_id: None,
            title: "Synthetic CPU fixture".into(),
            is_final: true,
        });
        store.save_job(&job).unwrap();
        let worker = MediaWorker::default();
        let shutdown = AtomicBool::new(false);
        let first = worker
            .get(
                &store,
                root.path(),
                &job.id,
                &job.outputs[0].file,
                Some(&ffmpeg),
                &shutdown,
            )
            .unwrap();
        assert_eq!((first.width, first.height, first.fps), (320, 180, 20.0));
        assert_eq!(first.duration_seconds, 2.0);
        let poster = PathBuf::from(first.poster_path.as_ref().unwrap());
        assert!(poster.is_file());
        let previous = stamp(&poster).unwrap();
        let second = worker
            .get(
                &store,
                root.path(),
                &job.id,
                &job.outputs[0].file,
                Some(&ffmpeg),
                &shutdown,
            )
            .unwrap();
        assert_eq!(second.poster_path, first.poster_path);
        assert_eq!(stamp(&poster).unwrap(), previous);
        assert!(worker
            .get(
                &store,
                root.path(),
                &job.id,
                "unregistered.mp4",
                Some(&ffmpeg),
                &shutdown
            )
            .is_err());
        fixture("3");
        let third = worker
            .get(
                &store,
                root.path(),
                &job.id,
                &job.outputs[0].file,
                Some(&ffmpeg),
                &shutdown,
            )
            .unwrap();
        assert_eq!(third.duration_seconds, 3.0);
        assert_ne!(third.poster_path, first.poster_path);
        let outside = root.path().join("outside.mp4");
        fs::write(&outside, b"outside").unwrap();
        job.outputs[0].file = outside.to_string_lossy().into_owned();
        store.save_job(&job).unwrap();
        assert!(worker
            .get(
                &store,
                root.path(),
                &job.id,
                &job.outputs[0].file,
                Some(&ffmpeg),
                &shutdown
            )
            .is_err());
    }

    #[test]
    #[ignore = "requires the bundled CPU FFmpeg via CLIPFORGE_TEST_FFMPEG"]
    fn local_playlist_cannot_open_a_network_secondary_input() {
        use crate::store::JobOutput;
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::atomic::AtomicUsize;
        let ffmpeg = PathBuf::from(std::env::var_os("CLIPFORGE_TEST_FFMPEG").unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let count = Arc::new(AtomicUsize::new(0));
        let stop = stopped.clone();
        let requests = count.clone();
        let server = std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        requests.fetch_add(1, Ordering::SeqCst);
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                        let mut buffer = [0; 4096];
                        let _ = stream.read(&mut buffer);
                        let _=stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
        });
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path().into()).unwrap();
        let mut job = store.new_job("demo", vec![], Default::default()).unwrap();
        let file = store.job_dir(&job.id).join("untrusted.mp4");
        fs::write(&file,format!("#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1.0,\nhttp://{address}/segment.ts\n#EXT-X-ENDLIST\n")).unwrap();
        job.outputs.push(JobOutput {
            file: file.to_string_lossy().into_owned(),
            bytes: fs::metadata(&file).unwrap().len(),
            highlight_id: None,
            title: "Playlist rejection fixture".into(),
            is_final: true,
        });
        store.save_job(&job).unwrap();
        let result = MediaWorker::default().get(
            &store,
            root.path(),
            &job.id,
            &job.outputs[0].file,
            Some(&ffmpeg),
            &AtomicBool::new(false),
        );
        stopped.store(true, Ordering::SeqCst);
        server.join().unwrap();
        assert!(result.is_err());
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "A registered local file must not cause a secondary network request."
        );
    }
}
