"""Container-only bounded media canonicalization; never import on the desktop host."""
from __future__ import annotations
import hashlib
import json
import math
from pathlib import Path
import struct
import subprocess
import sys


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024**2), b""):
            result.update(block)
    return result.hexdigest()


def run(command):
    return subprocess.run(command, check=True, capture_output=True, timeout=180, stdin=subprocess.DEVNULL)


def main():
    source, output = Path("/input.blob"), Path("/output")
    request = json.loads(Path("/request.json").read_text())
    if source.stat().st_size != request["sizeBytes"] or not 0 < source.stat().st_size <= 2 * 1024**3 or digest(source) != request["sha256"]:
        raise ValueError("Registered source hash or size changed.")
    probe = json.loads(run(["ffprobe", "-v", "error", "-protocol_whitelist", "file", "-show_entries", "format=duration:stream=codec_type,width,height", "-of", "json", str(source)]).stdout)
    videos = [stream for stream in probe.get("streams", []) if stream.get("codec_type") == "video"]
    duration = float(probe.get("format", {}).get("duration", 0))
    if len(videos) != 1 or not math.isfinite(duration) or not 0.1 <= duration <= 120 or not 1 <= videos[0].get("width", 0) <= 8192 or not 1 <= videos[0].get("height", 0) <= 8192:
        raise ValueError("Only a single bounded video up to two minutes is supported.")
    sanitized = output / "example.mp4"
    command = ["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-threads", "1", "-filter_threads", "1", "-protocol_whitelist", "file", "-i", str(source), "-map", "0:v:0", "-vf", "scale=960:540:force_original_aspect_ratio=decrease,pad=960:540:(ow-iw)/2:(oh-ih)/2,fps=15", "-c:v", "libx264", "-preset", "veryfast", "-crf", "24", "-t", "120", "-fs", str(128 * 1024**2), "-sn", "-dn", "-map_metadata", "-1"]
    command += ["-map", "0:a:0?", "-c:a", "aac", "-b:a", "96k"] if request.get("includeAudio") else ["-an"]
    run(command + ["-movflags", "+faststart", str(sanitized)])
    frames = []
    count = min(16, max(2, math.ceil(duration * 2)))
    for index in range(count):
        timestamp = index * duration / count
        frame = output / f"frame-{index:02d}.png"
        run(["ffmpeg", "-nostdin", "-hide_banner", "-loglevel", "error", "-threads", "1", "-filter_threads", "1", "-ss", str(timestamp), "-i", str(sanitized), "-frames:v", "1", "-map_metadata", "-1", "-vf", "scale=640:360", "-compression_level", "6", str(frame)])
        if frame.stat().st_size > 1024**2:
            raise ValueError("Canonical frame exceeds one MiB.")
        frames.append({"name": frame.name, "sha256": digest(frame), "timestamp": timestamp, "sizeBytes": frame.stat().st_size})
    receipt = {"schemaVersion": 1, "sourceId": request["sourceId"], "inputSha256": request["sha256"], "sandbox": "rootless-no-network-v1", "duration": duration, "mediaSha256": digest(sanitized), "mediaBytes": sanitized.stat().st_size, "frames": frames, "audioIncluded": bool(request.get("includeAudio"))}
    (output / "receipt.json").write_text(json.dumps(receipt, sort_keys=True), encoding="utf-8")
    # The container receives no writable host mount. Transfer only declared,
    # bounded byte streams; the host does not parse an archive or decode media.
    header = json.dumps(receipt, sort_keys=True).encode()
    destination = sys.stdout.buffer
    destination.write(b"CFV1" + struct.pack("!I", len(header)) + header)
    for path in [sanitized] + [output / frame["name"] for frame in frames]:
        with path.open("rb") as source_file:
            for block in iter(lambda: source_file.read(65536), b""):
                destination.write(block)
    destination.flush()


if __name__ == "__main__":
    main()
