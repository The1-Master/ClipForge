# ClipForge private learning worker

This is an operator tool, not an automatic background trainer. Feedback does not upload footage. Teacher predictions remain proposals; human labels are retained separately. CPU preference calibration changes ranking, not Qwen weights. No trained neural quality claim is made until independent whole-match evidence passes the release gate.

## CPU isolation first

Use a dedicated WSL2 distribution called `ClipForge-Training`, imported with its VHDX below `J:\CS2ClipStudio`. Install Podman, uidmap, slirp4netns, Python 3 and unprivileged-user support inside that distribution. Do not use the stopped personal Ubuntu distribution or alter its filesystem. Its system packages and container storage then live in the workspace-backed VHDX; application evidence, logs and model caches remain below the mounted workspace.

Run as the unprivileged distribution user:

```text
python3 /mnt/j/CS2ClipStudio/app/training/setup_cpu.py --root /mnt/j/CS2ClipStudio --distro ClipForge-Training
```

The setup builds the local validator image, pins its immutable digest, generates a trusted two-second fixture inside a network-disabled container, and exercises the real canonicalization path. Only successful isolation writes `sandboxAcceptancePassed`. The decoder worker has no network, no capabilities, no writable host mount, a read-only root, bounded output tmpfs, 768 MiB RAM, one CPU, 32 processes and a five-minute limit. Checked names, lengths and hashes cross a bounded byte protocol; the host does not parse an archive or decode media. Parent EOF cleans only its nonce-named container.

Downloaded media stays opaque and checksum-verified in quarantine before validation. The worker accepts one video up to two minutes, emits a stripped 960×540/15 FPS H.264 review copy and bounded canonical PNGs. Long clips can be reviewed by people, but AI teacher/event training accepts grounded windows no longer than eight seconds. Raw demos remain quarantined until a separate isolated demo-to-example worker is implemented; the beta never parses contributed raw demos on the Windows host.

## Neural hardware acceptance

AMD documents RX 9070 XT support with PyTorch 2.9.1/ROCm 7.2.1 on WSL. That is hardware/framework support, not certification of this Qwen fine-tune. Obtain AMD's official wheel, verify its checksum, and save it under the workspace. `setup_neural.py --root ... --torch-wheel ... --torch-sha256 ...` installs an isolated environment; it does not certify training or change graphics drivers.

The trainer requires the pinned official `Qwen/Qwen3-VL-4B-Instruct` revision `ebb281ec70b05090aa6165b016eac8ec08e71b17` (Apache-2.0) safetensors. Existing Ollama Q4 files are inference assets and are rejected for training.

```text
tools/training-env/bin/python app/training/trainer.py --root /mnt/j/CS2ClipStudio prepare
tools/training-env/bin/python app/training/trainer.py --root /mnt/j/CS2ClipStudio probe --image <trusted canonical 640x360 PNG> --output <workspace pilot directory>
tools/training-env/bin/python app/training/accept_probe.py --root /mnt/j/CS2ClipStudio --probe <workspace pilot directory>
```

Preparation retains a 20 GiB free-space reserve and requires another 12 GiB for source weights. Runtime installation and merging need additional headroom. The pilot uses language-attention LoRA rank 8, frozen vision/projector/base weights, batch one, SDPA, FP16 with gradient scaling and checkpointing. It proves nonzero finite gradients, an optimizer change, a saved adapter and actual fresh-base checkpoint reload. GPU inference success alone cannot enable training.

When started through ClipForge, the compute lease prevents concurrent recording/review. The parent pipe and Linux supervisor terminate the owned session on cancellation; a five-second TERM grace is followed by KILL. An external CS2 process stops training. No unrelated game, distribution or container is terminated.

## Dataset, evaluation and release

Trainer credentials are independent revocable server credentials protected by the existing local credential store. The receiver issues immutable approved snapshots and a deletion ledger. The controller checks tombstones before downloading, before fitting, after fitting and before promotion. Withdrawals remove quarantine, canonical frames, teacher proposals and reviewer mirrors. Previously released weights cannot be promised to forget data instantly; affected new checkpoints cannot be promoted.

Contributor pseudonyms and source-match hashes are mandatory. Connected contributors and matches are kept together in the train/test split. Unknown legacy provenance is excluded. Positive neural examples additionally require independent human POV verification, 2–4 specific evidence frames, matching timestamps, event bounds and certainty. A keep rating alone never invents those labels. Current reviewer ratings can train a CPU preference ranker; detailed visual grounding needs independent annotation before neural fitting.

The private `ground.py` CLI makes that annotation actionable today. Prepare/open the canonical example and submit its keep/timing label in the reviewer UI, then have the assigned independent human reviewer create this bounded JSON inside the workspace. Use the exact payload hash, receipt hash and current label revision from the private snapshot; tick bounds must match that reviewed label, and selected frame timestamps must lie inside the specified offsets:

```json
{"schemaVersion":1,"id":"<32 hex example id>","sourceSha256":"<64 hex>","labelRevision":1,"canonicalReceiptSha256":"<64 hex>","visualGrounding":{"povVerified":true,"evidenceFrames":[2,3],"startOffset":1.0,"endOffset":3.0,"startTick":100,"endTick":200,"confidence":95}}
```

Run on Windows with the workspace's Python: `training/ground.py --root J:\CS2ClipStudio --annotation <workspace JSON path>`. The independent reviewer credential is entered through a masked prompt, or a bounded private stdin pipe with `--credential-stdin`; it never enters argv, logs or exported metadata. The server verifies the reviewer role and assignment. The stored human assertion is bound to the exact canonical receipt, source hash and label revision, is invalidated by changes and is erased on withdrawal. Teacher predictions cannot call this human grounding path. The grounded assertions are consumed by the next immutable training snapshot.

Each candidate version keeps its dataset snapshot, immutable artifact hash and private results. `evaluate.py` computes metrics from versioned whole-match human annotations and scores generated by the exact ranker or by actual private checkpoint inference. Imported aggregate score reports cannot pass. Coverage must be explicitly human-reviewed. It checks contributor/match leakage, a prior-model comparison and certified native export witnesses. Promotion requires at least five contributors, ten matches, 50 positives/50 negatives, 90% must-keep recall, 80% top-20 precision, no wrong-player exports and no baseline regression. Categories with fewer than 20 positives are reported as insufficient evidence.

Benchmark export entries use `jobPath` (registered `job.json`) and `outputKey`, not claimed clip hashes or a success checkbox. Evaluation and promotion recheck the actual original demo fingerprint, registered output bytes/hash and measured native timeline including every frame's personal POV. Duplicate copies cannot inflate the export count or suggestion precision.

`trainer.py merge` writes merged safetensors; `gguf` uses an explicitly pinned llama.cpp converter to export the language model and matching vision projector. That does not prove Ollama import support. An actual private ClipForge/Ollama vision-schema reload probe and exact model manifest digest must populate the compatibility record before neural evaluation/promotion. Active neural aliases are restricted to `clipforge-qwen3-vl:<version-id>`. The previous model and builtin fallback remain available.

Primary references: [official Qwen model](https://huggingface.co/Qwen/Qwen3-VL-4B-Instruct), [AMD WSL support matrix](https://rocm.docs.amd.com/projects/radeon-ryzen/en/latest/docs/compatibility/compatibilityrad/wsl/wsl_compatibility.html), [official Qwen fine-tuning implementation](https://github.com/QwenLM/Qwen3-VL/tree/main/qwen-vl-finetune).

## Actual CPU worker verification

`worker_audit.py --root /mnt/j/CS2ClipStudio` runs only in the existing unprivileged dedicated distribution. It generates trusted test footage inside a CPU container, closes the parent's input pipe while a nonce-named validator is running, tests the bounded timeout and malformed media, and exercises the actual file-size and aggregate tmpfs limits. It preserves unrelated containers and writes evidence below `research/benchmarks/community-learning/worker`.

The October 13, 2026 run passed: parent-EOF cleanup 4.148 seconds, one-second processing-budget rejection and cleanup 7.774 seconds, zero partial outputs, unrelated sentinel preserved, malformed media rejected without a receipt, file-size rejection at 128 MiB and aggregate output rejection at 256 MiB. Actual container inspection confirmed rootless operation, one CPU, 768 MiB RAM, 32 processes, read-only root, no network and no writable host mount. No GPU training or model-accuracy conclusion follows from these CPU checks.
