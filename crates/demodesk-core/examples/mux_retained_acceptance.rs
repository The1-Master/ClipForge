//! CPU-only native validation/mux of a retained source, truthfully labeled1080p.
use anyhow::{anyhow, ensure, Result};
use demodesk_core::render::{collect_clip_outputs, encode::{mux_clip, validate_native_capture, Control}, lease::ComputeLease};
use serde_json::json;
use std::{fs, path::{Path, PathBuf}, sync::atomic::AtomicBool};

fn main() -> Result<()> {
    let root=Path::new("J:/CS2ClipStudio");
    let folder=PathBuf::from(std::env::args().nth(1).ok_or_else(||anyhow!("benchmark folder required"))?);
    ensure!(folder.canonicalize()?.starts_with(root.join("research/benchmarks/render").canonicalize()?),"source must stay in J: research");
    let job:serde_json::Value=serde_json::from_slice(&fs::read(folder.join("result.json"))?)?;
    ensure!(job["status"]=="error","this diagnostic must preserve an originally failed4K job");
    let id=job["id"].as_str().ok_or_else(||anyhow!("missing job ID"))?;
    ensure!(id.chars().all(|ch|ch.is_ascii_alphanumeric()||ch=='-'),"invalid job ID");
    let pass=folder.join("state/clips").join(id).join("pass-1");
    let clip=collect_clip_outputs(&pass,1,"mp4").into_iter().next().ok_or_else(||anyhow!("missing retained source"))?;
    let video=clip.video.as_ref().ok_or_else(||anyhow!("missing retained video"))?;
    ensure!(clip.audio.is_some(),"missing captured game audio");
    let destination=folder.join("diagnostic-personal-1080p60.mp4");
    let ffmpeg=root.join("tools/ffmpeg/ffmpeg-master-latest-win64-gpl/bin/ffmpeg.exe");
    let cancel=AtomicBool::new(false);
    let _lease=ComputeLease::try_acquire()?.ok_or_else(||anyhow!("compute slot is busy; retry after it is released"))?;
    let mut log=|line|println!("{line}");
    let mut control=Control::new(&cancel,&mut log);
    validate_native_capture(&ffmpeg,video,1920,1080,60,10.0,64.0,&mut control)?;
    mux_clip(&ffmpeg,&clip,&destination,192,&mut control)?;
    let sidecar=PathBuf::from(format!("{}.timeline.json",destination.display()));
    let timeline:serde_json::Value=serde_json::from_slice(&fs::read(&sidecar)?)?;
    ensure!(timeline["validated"]==true,"native timeline publication did not validate the actual1080p source");
    println!("{}",json!({"video":destination,"sidecar":sidecar,"width":1920,"height":1080,"fps":60,
        "frameCount":timeline["frameCount"],"encodedFrameCount":timeline["encodedFrameCount"],
        "videoSHA256":timeline["videoSHA256"],"validated":timeline["validated"],"original4KJobStatusUnchanged":"error"}));
    Ok(())
}
