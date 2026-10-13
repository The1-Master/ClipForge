"""Record acceptance only from an actual finite backward/checkpoint/reload probe."""
from __future__ import annotations
import argparse
import hashlib
import json
import math
from pathlib import Path
import re


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True)
    parser.add_argument("--probe", required=True)
    args = parser.parse_args()
    root, probe = Path(args.root).resolve(strict=True), Path(args.probe).resolve(strict=True)
    if not probe.is_relative_to(root):
        raise ValueError("The hardware probe must belong to the workspace.")
    result = json.loads((probe / "fit-result.json").read_text())
    adapter = probe / "adapter/adapter_model.safetensors"
    hasher = hashlib.sha256()
    with adapter.open("rb") as source:
        for block in iter(lambda: source.read(1024**2), b""):
            hasher.update(block)
    gradients = result.get("gradientNorms", [])
    losses = result.get("losses", [])
    if result.get("kind") != "hardware-pilot" or result.get("model") != "Qwen/Qwen3-VL-4B-Instruct" or result.get("revision") != "ebb281ec70b05090aa6165b016eac8ec08e71b17" or result.get("finiteBackward") is not True or result.get("checkpointReloaded") is not True or hasher.hexdigest() != result.get("adapterSha256") or not gradients or not losses or any(type(value) not in (int, float) or not math.isfinite(value) for value in gradients + losses) or not any(value > 0 for value in gradients) or not result.get("hipVersion") or not 0 < result.get("parameterDeltaL1", 0) or not 0 <= result.get("reloadLossDelta", 100) <= .05:
        raise ValueError("The actual AMD finite backward, changed adapter and checkpoint reload acceptance is incomplete.")
    state = root / "state/learning/environment.json"
    config = json.loads(state.read_text())
    config.update({"rocmBackwardPassed": True, "rocmAcceptance": {"path": str(probe), "resultSha256": hashlib.sha256((probe / "fit-result.json").read_bytes()).hexdigest(), "adapterSha256": hasher.hexdigest(), "qualityEvaluated": False}})
    state.write_text(json.dumps(config, sort_keys=True))
    print(json.dumps({"state": "hardware_accepted", "qualityEvaluated": False}))


if __name__ == "__main__":
    main()
