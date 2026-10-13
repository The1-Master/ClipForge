"""Create and exercise a rootless media validator; no GPU or untrusted media here."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    parser.add_argument("--distro", default="ClipForge-Training")
    args = parser.parse_args()
    root = Path(args.root).resolve(strict=True)
    if os.getuid() == 0 or not str(root).startswith("/mnt/"):
        raise RuntimeError("Run this setup as the unprivileged user in the dedicated workspace-backed WSL distribution.")
    result = subprocess.run(["podman", "info", "--format", "{{.Host.Security.Rootless}}"], check=True, capture_output=True, text=True, timeout=20)
    if result.stdout.strip() != "true":
        raise RuntimeError("Rootless Podman is not configured.")
    training = root / "app/training"
    subprocess.run(["podman", "build", "--tag", "localhost/clipforge-validator:1", "--file", str(training / "Containerfile.validator"), str(training)], check=True, timeout=1200)
    image = subprocess.run(["podman", "image", "inspect", "localhost/clipforge-validator:1", "--format", "{{.Id}}"], check=True, capture_output=True, text=True, timeout=20).stdout.strip()
    if not image.startswith("sha256:"):
        image = "sha256:" + image
    folder = root / "state/learning/sandbox-acceptance"
    folder.mkdir(parents=True, exist_ok=True)
    source = folder / "fixture.mp4"
    if not source.exists():
        subprocess.run(["podman", "run", "--rm", "--network=none", "--read-only", "--cap-drop=ALL", "--security-opt=no-new-privileges", "--memory=256m", "--cpus=1", "--pids-limit=32", "--userns=keep-id", "--user", f"{os.getuid()}:{os.getgid()}", "--entrypoint=ffmpeg", "-v", str(folder) + ":/out:rw", image, "-nostdin", "-loglevel", "error", "-threads", "1", "-filter_threads", "1", "-f", "lavfi", "-i", "testsrc=size=640x360:rate=15", "-t", "2", "-c:v", "libx264", "-pix_fmt", "yuv420p", "/out/fixture.mp4"], check=True, timeout=90)
    identifier = uuid.uuid4().hex
    request = {"sourceId": identifier, "sha256": hashlib.sha256(source.read_bytes()).hexdigest(), "sizeBytes": source.stat().st_size, "includeAudio": False, "kind": "example"}
    request_path = folder / "fixture.request.json"
    request_path.write_text(json.dumps(request))
    output = folder / identifier
    subprocess.run([sys.executable, str(training / "sandbox.py"), "--root", str(root), "--input", str(source), "--request", str(request_path), "--output", str(output), "--image", image], check=True, timeout=360)
    receipt = json.loads((output / "receipt.json").read_text())
    if receipt.get("inputSha256") != request["sha256"] or receipt.get("sandbox") != "rootless-no-network-v1" or receipt.get("audioIncluded") is not False:
        raise RuntimeError("The actual CPU isolation acceptance run failed.")
    state = root / "state/learning/environment.json"
    config = json.loads(state.read_text()) if state.exists() else {}
    config.update({"schemaVersion": 1, "wslDistro": args.distro, "sandboxPython": sys.executable, "sandboxImage": image, "sandboxAcceptancePassed": True, "sandboxAcceptance": {"sourceSha256": request["sha256"], "receiptSha256": hashlib.sha256((output / "receipt.json").read_bytes()).hexdigest(), "workerSha256": hashlib.sha256((training / "validator_worker.py").read_bytes()).hexdigest()}, "rocmBackwardPassed": config.get("rocmBackwardPassed", False)})
    state.write_text(json.dumps(config, sort_keys=True))
    print(json.dumps({"state": "sandbox_ready", "image": image, "gpuTrainingCertified": False}))


if __name__ == "__main__":
    main()
