"""Workspace-only neural environment setup using an explicitly verified AMD wheel.

This installs dependencies; it does not launch the GPU or certify training.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import venv


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    parser.add_argument("--torch-wheel", required=True)
    parser.add_argument("--torch-sha256", required=True)
    args = parser.parse_args()
    root = Path(args.root).resolve(strict=True)
    wheel = Path(args.torch_wheel).resolve(strict=True)
    if os.name != "posix" or not wheel.is_relative_to(root) or not wheel.name.startswith("torch-2.9.1") or wheel.suffix != ".whl" or not re.fullmatch(r"[a-f0-9]{64}", args.torch_sha256):
        raise ValueError("Use an independently checked official AMD ROCm PyTorch 2.9.1 wheel saved in the workspace.")
    hasher = hashlib.sha256()
    with wheel.open("rb") as source:
        for block in iter(lambda: source.read(1024**2), b""):
            hasher.update(block)
    if hasher.hexdigest() != args.torch_sha256:
        raise ValueError("The AMD PyTorch wheel checksum does not match.")
    target = root / "tools/training-env"
    venv.EnvBuilder(with_pip=True).create(target)
    python = target / "bin/python"
    environment = os.environ.copy()
    for name, relative in (("PIP_CACHE_DIR", "cache/pip-training"), ("TMPDIR", "temp/training"), ("XDG_CACHE_HOME", "cache/training")):
        path = root / relative
        path.mkdir(parents=True, exist_ok=True)
        environment[name] = str(path)
    environment["PYTHONNOUSERSITE"] = "1"
    subprocess.run([str(python), "-m", "pip", "install", str(wheel)], env=environment, check=True, timeout=1800)
    subprocess.run([str(python), "-m", "pip", "install", "-r", str(root / "app/training/requirements.txt")], env=environment, check=True, timeout=1800)
    state = root / "state/learning/environment.json"
    config = json.loads(state.read_text()) if state.exists() else {}
    config.update({"schemaVersion": 1, "linuxPython": str(python), "torchWheelSha256": args.torch_sha256, "rocmBackwardPassed": False})
    state.parent.mkdir(parents=True, exist_ok=True)
    state.write_text(json.dumps(config, sort_keys=True))
    print(json.dumps({"state": "installed", "trainingCertified": False, "next": "Prepare pinned Qwen safetensors, then run an explicit finite backward/checkpoint/reload probe with a trusted canonical fixture."}))


if __name__ == "__main__":
    main()
