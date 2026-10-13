"""Run the canonical media decoder only inside a rootless, network-disabled container."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import select
import signal
import struct
import subprocess
import sys
import threading
import time
import uuid

ROOT = Path("/mnt/j/CS2ClipStudio").resolve()


def owned(value, exists=False):
    path = Path(value).resolve()
    if not path.is_relative_to(ROOT) or (exists and not path.is_file()):
        raise ValueError("The sandbox only accepts workspace-owned files.")
    return path


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as source:
        for data in iter(lambda: source.read(1024**2), b""):
            result.update(data)
    return result.hexdigest()


def validate_result(output, request):
    receipt = json.loads((output / "receipt.json").read_text())
    if receipt.get("schemaVersion") != 1 or receipt.get("sourceId") != request["sourceId"] or receipt.get("inputSha256") != request["sha256"] or receipt.get("sandbox") != "rootless-no-network-v1":
        raise ValueError("Invalid canonicalization receipt.")
    frames = receipt.get("frames", [])
    if not 2 <= len(frames) <= 16:
        raise ValueError("Invalid canonical frame count.")
    files = list(output.iterdir())
    if any(path.is_symlink() or not path.is_file() for path in files) or sum(path.stat().st_size for path in files) > 256 * 1024**2:
        raise ValueError("Canonical output exceeds supported bounds.")
    if {path.name for path in files} != {"example.mp4", "receipt.json"} | {frame["name"] for frame in frames}:
        raise ValueError("Unexpected canonical output files.")
    for frame in frames:
        if not re.fullmatch(r"frame-\d{2}\.png", frame.get("name", "")):
            raise ValueError("Invalid canonical frame name.")
        path = output / frame["name"]
        if path.stat().st_size > 1024**2 or digest(path) != frame["sha256"]:
            raise ValueError("Canonical frame checksum mismatch.")
    if digest(output / "example.mp4") != receipt.get("mediaSha256"):
        raise ValueError("Canonical media checksum mismatch.")
    return receipt


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", default="/mnt/j/CS2ClipStudio")
    parser.add_argument("--input", required=True)
    parser.add_argument("--request", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--parent-pipe", action="store_true")
    parser.add_argument("--time-budget", type=int, default=300)
    args = parser.parse_args()
    if not 1 <= args.time_budget <= 300:
        raise ValueError("Choose a bounded validator time budget from one to 300 seconds.")
    global ROOT
    ROOT = Path(args.root).resolve(strict=True)
    if not str(ROOT).startswith("/mnt/") or not ROOT.is_dir():
        raise ValueError("Choose the mounted ClipForge workspace.")
    if os.getuid() == 0:
        raise RuntimeError("Run the validator as an unprivileged Linux user, never root.")
    if not re.fullmatch(r"sha256:[a-f0-9]{64}", args.image):
        raise ValueError("Use a locally built, pinned image digest.")
    source, request_path, output = owned(args.input, True), owned(args.request, True), owned(args.output)
    if request_path.stat().st_size > 8192:
        raise ValueError("Oversized validator request.")
    request = json.loads(request_path.read_text())
    if not re.fullmatch(r"[a-f0-9]{32}", request.get("sourceId", "")) or source.stat().st_size != request.get("sizeBytes") or digest(source) != request.get("sha256"):
        raise ValueError("Input checksum does not match the registered payload.")
    if request.get("kind") != "example":
        raise ValueError("Raw demos remain quarantined; this worker only validates selected media examples.")
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        print(json.dumps(validate_result(output, request)))
        return
    rootless = subprocess.run(["podman", "info", "--format", "{{.Host.Security.Rootless}}"], check=True, capture_output=True, text=True, timeout=20)
    if rootless.stdout.strip() != "true":
        raise RuntimeError("A verified rootless Podman runtime is required.")
    name = "clipforge-validate-" + uuid.uuid4().hex
    def interrupted(_signal, _frame):
        raise InterruptedError("The owned validator was cancelled.")
    signal.signal(signal.SIGTERM, interrupted)
    if args.parent_pipe:
        def watch():
            sys.stdin.buffer.read(1)
            os.kill(os.getpid(), signal.SIGTERM)
        threading.Thread(target=watch, name="validator-parent", daemon=True).start()
    command = ["podman", "run", "--name", name, "--label", "clipforge.owner=" + name, "--rm", "--pull=never", "--network=none", "--read-only", "--cap-drop=ALL", "--security-opt=no-new-privileges", "--userns=keep-id", "--user", f"{os.getuid()}:{os.getgid()}", "--memory=768m", "--memory-swap=768m", "--cpus=1", "--pids-limit=32", "--ulimit=nofile=64:64", "--ulimit=fsize=134217728:134217728", "--tmpfs=/tmp:rw,size=128m,noexec,nosuid,nodev", "--tmpfs=/output:rw,size=256m,noexec,nosuid,nodev,mode=1777", "-v", str(source) + ":/input.blob:ro", "-v", str(request_path) + ":/request.json:ro", args.image]
    process = None
    created = []
    try:
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, bufsize=0)
        deadline = time.monotonic() + args.time_budget
        def read_exact(size):
            chunks, received = [], 0
            while received < size:
                if time.monotonic() >= deadline:
                    raise TimeoutError("The isolated validator exceeded its time budget.")
                if not select.select([process.stdout], [], [], min(1, max(.01, deadline - time.monotonic())))[0]:
                    continue
                block = os.read(process.stdout.fileno(), min(65536, size - received))
                if not block:
                    raise ValueError("The isolated validator ended before its bounded output completed.")
                chunks.append(block)
                received += len(block)
            return b"".join(chunks)
        if read_exact(4) != b"CFV1":
            raise ValueError("Invalid isolated validator protocol.")
        header_size = struct.unpack("!I", read_exact(4))[0]
        if not 0 < header_size <= 16384:
            raise ValueError("Oversized isolated validator receipt.")
        receipt = json.loads(read_exact(header_size))
        if receipt.get("sourceId") != request["sourceId"] or receipt.get("inputSha256") != request["sha256"] or receipt.get("sandbox") != "rootless-no-network-v1" or type(receipt.get("mediaBytes")) is not int or not 0 < receipt["mediaBytes"] <= 128 * 1024**2:
            raise ValueError("Invalid isolated validator output identity or bounds.")
        frames = receipt.get("frames", [])
        if not isinstance(frames, list) or not 2 <= len(frames) <= 16:
            raise ValueError("Invalid isolated frame count.")
        declared = [{"name": "example.mp4", "sizeBytes": receipt["mediaBytes"], "sha256": receipt.get("mediaSha256")}] + frames
        names = {"example.mp4"}
        for frame in frames:
            if not re.fullmatch(r"frame-\d{2}\.png", str(frame.get("name", ""))) or frame["name"] in names or type(frame.get("sizeBytes")) is not int or not 0 < frame["sizeBytes"] <= 1024**2:
                raise ValueError("Invalid isolated frame identity or bounds.")
            names.add(frame["name"])
        for item in declared:
            if not re.fullmatch(r"[a-f0-9]{64}", str(item.get("sha256", ""))):
                raise ValueError("Invalid isolated output hash.")
            target = output / item["name"]
            hasher, remaining = hashlib.sha256(), item["sizeBytes"]
            with target.open("xb") as destination:
                created.append(target)
                while remaining:
                    block = read_exact(min(65536, remaining))
                    hasher.update(block)
                    destination.write(block)
                    remaining -= len(block)
            if hasher.hexdigest() != item["sha256"]:
                raise ValueError("The isolated output hash did not match its receipt.")
        process.wait(timeout=10)
        if process.returncode or process.stdout.read(1):
            raise ValueError("The isolated validator failed or produced undeclared data.")
        (output / "receipt.json").write_text(json.dumps(receipt, sort_keys=True), encoding="utf-8")
    except BaseException:
        # A cancelled transfer is retryable. Remove only files this supervisor
        # itself created through the bounded protocol, never pre-existing files.
        for target in created:
            target.unlink(missing_ok=True)
        raise
    finally:
        # Only this nonce-named container is cleaned up. An unrelated podman
        # service/container or the whole WSL distribution is never stopped.
        subprocess.run(["podman", "rm", "--force", "--time=3", name], timeout=15, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if process and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
    print(json.dumps(validate_result(output, request)))


if __name__ == "__main__":
    main()
