"""Actual CPU-only validator cancellation, isolation and malicious-input smoke tests."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid


def run(command, **kwargs):
    return subprocess.run(command, check=True, capture_output=True, text=True, timeout=60, **kwargs)


def names():
    return set(run(["podman", "ps", "--format", "{{.Names}}"]).stdout.splitlines())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    args = parser.parse_args()
    root = Path(args.root).resolve(strict=True)
    if os.getuid() == 0:
        raise RuntimeError("Use the unprivileged dedicated distribution user.")
    config = json.loads((root / "state/learning/environment.json").read_text())
    image = config["sandboxImage"]
    folder = root / "research/benchmarks/community-learning/worker" / uuid.uuid4().hex
    folder.mkdir(parents=True)
    fixture = root / "state/learning/sandbox-acceptance/fixture.mp4"
    longer = folder / "trusted-long.mp4"
    common = ["podman", "run", "--rm", "--network=none", "--read-only", "--cap-drop=ALL", "--security-opt=no-new-privileges", "--memory=512m", "--memory-swap=512m", "--cpus=1", "--pids-limit=32", "--userns=keep-id", "--user", f"{os.getuid()}:{os.getgid()}"]
    # Generate test media entirely inside the CPU container from a trusted fixture.
    run(common + ["--entrypoint=ffmpeg", "-v", str(fixture) + ":/fixture.mp4:ro", "-v", str(folder) + ":/out:rw", image, "-nostdin", "-loglevel", "error", "-stream_loop", "39", "-i", "/fixture.mp4", "-c", "copy", "/out/trusted-long.mp4"])
    sentinel = "clipforge-audit-sentinel-" + uuid.uuid4().hex
    run(["podman", "run", "--detach", "--name", sentinel, "--network=none", "--read-only", "--cap-drop=ALL", "--memory=16m", "--pids-limit=8", "--entrypoint=sleep", image, "120"])
    report = {"schemaVersion": 1, "cpuOnly": True, "workspace": str(root), "image": image, "results": {}}
    before = names()
    try:
        def launch(source, budget=300, lifeline=True):
            identifier = uuid.uuid4().hex
            request = {"sourceId": identifier, "sha256": hashlib.sha256(source.read_bytes()).hexdigest(), "sizeBytes": source.stat().st_size, "includeAudio": False, "kind": "example"}
            request_path = folder / (identifier + ".request.json")
            request_path.write_text(json.dumps(request))
            output = folder / identifier
            command = [sys.executable, str(root / "app/training/sandbox.py"), "--root", str(root), "--input", str(source), "--request", str(request_path), "--output", str(output), "--image", image, "--time-budget", str(budget)]
            if lifeline:
                command.append("--parent-pipe")
            log = (folder / (identifier + ".log")).open("wb")
            return subprocess.Popen(command, stdin=subprocess.PIPE, stdout=log, stderr=log), log, output
        process, log, output = launch(longer)
        active = None
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline and process.poll() is None:
            active = next((name for name in names() - before if name.startswith("clipforge-validate-")), None)
            if active:
                break
            time.sleep(.1)
        if not active:
            process.stdin.close()
            process.wait(timeout=15)
            raise RuntimeError("The actual validator never reached a running container; cancellation is unproven.")
        inspection = json.loads(run(["podman", "inspect", active]).stdout)[0]
        report["limits"] = {key: inspection["HostConfig"].get(key) for key in ("Memory", "MemorySwap", "NanoCpus", "CpuQuota", "CpuPeriod", "PidsLimit", "ReadonlyRootfs", "NetworkMode", "Tmpfs", "Ulimits", "CapDrop")}
        report["limits"]["rootless"] = run(["podman", "info", "--format", "{{.Host.Security.Rootless}}"]).stdout.strip() == "true"
        report["limits"]["writableHostMounts"] = [mount["Destination"] for mount in inspection.get("Mounts", []) if mount.get("Type") == "bind" and mount.get("RW") is True]
        began = time.monotonic()
        process.stdin.close()
        process.wait(timeout=10)
        elapsed = time.monotonic() - began
        after = names()
        log.close()
        if process.returncode == 0 or active in after or sentinel not in after or not (before - {sentinel}) <= after or elapsed > 10:
            raise RuntimeError("Actual parent EOF did not safely stop only its owned container.")
        report["results"]["parentEOF"] = {"passed": True, "elapsedSeconds": elapsed, "exitCode": process.returncode, "ownedContainerRemoved": True, "sentinelPreserved": True, "preExistingContainersPreserved": True, "partialOutputs": len(list(output.iterdir()))}
        process, log, output = launch(longer, budget=1, lifeline=False)
        began = time.monotonic()
        process.wait(timeout=12)
        log.close()
        error_text = (Path(log.name)).read_text(errors="replace")
        if process.returncode == 0 or "time budget" not in error_text or any(name.startswith("clipforge-validate-") for name in names() - before):
            raise RuntimeError("The actual isolated timeout did not reject and clean up.")
        report["results"]["timeout"] = {"passed": True, "elapsedSeconds": time.monotonic() - began, "exitCode": process.returncode, "partialOutputs": len(list(output.iterdir()))}
        malformed = folder / "malformed.blob"
        malformed.write_bytes(b"Not a video. CPU-only malformed-input fixture.\x00" * 100)
        process, log, output = launch(malformed, budget=30, lifeline=False)
        process.wait(timeout=40)
        log.close()
        if process.returncode == 0 or (output / "receipt.json").exists() or any(name.startswith("clipforge-validate-") for name in names() - before):
            raise RuntimeError("Malformed media was not rejected safely.")
        report["results"]["malformedMedia"] = {"passed": True, "exitCode": process.returncode, "receiptCreated": False}
        size_code = "import errno,json,os; p='/output/limit'; f=open(p,'wb'); n=0\ntry:\n while True:\n  f.write(b'x'*65536); n+=65536\nexcept OSError as e:\n f.close(); print(json.dumps({'errno':e.errno,'size':os.stat(p).st_size})); assert e.errno==errno.EFBIG and os.stat(p).st_size==134217728"
        result = run(common + ["--tmpfs=/output:rw,size=256m,noexec,nosuid,nodev,mode=1777", "--ulimit=fsize=134217728:134217728", "--entrypoint=python3", image, "-c", size_code])
        report["results"]["fileSizeLimit"] = {"passed": True, **json.loads(result.stdout)}
        tmpfs_code = "import errno,json,os; block=b'x'*65536\ntry:\n for i in range(3):\n  with open('/output/'+str(i),'wb') as f:\n   for j in range(1536): f.write(block)\nexcept OSError as e:\n print(json.dumps({'errno':e.errno,'totalBytes':sum(os.stat('/output/'+p).st_size for p in os.listdir('/output'))})); assert e.errno==errno.ENOSPC"
        result = run(common + ["--tmpfs=/output:rw,size=256m,noexec,nosuid,nodev,mode=1777", "--ulimit=fsize=134217728:134217728", "--entrypoint=python3", image, "-c", tmpfs_code])
        report["results"]["outputTmpfsLimit"] = {"passed": True, **json.loads(result.stdout)}
        limits = report["limits"]
        if limits["Memory"] != 768 * 1024**2 or limits["PidsLimit"] != 32 or limits["ReadonlyRootfs"] is not True or limits["NetworkMode"] != "none" or not limits["rootless"] or limits["writableHostMounts"]:
            raise RuntimeError("Actual isolation configuration differs from the required bounds.")
        report["passed"] = True
    finally:
        run(["podman", "rm", "--force", sentinel])
        report["remainingContainers"] = sorted(names())
        (folder / "verification.json").write_text(json.dumps(report, indent=2, sort_keys=True))
    print(json.dumps({"passed": report.get("passed", False), "report": str(folder / "verification.json"), "results": report["results"]}, sort_keys=True))


if __name__ == "__main__":
    main()
