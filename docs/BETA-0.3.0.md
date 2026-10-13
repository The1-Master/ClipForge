# ClipForge 0.3.0 beta verification

Verified on Windows on October 13, 2026. This beta adds event-centered discovery,
optional contribution workflows and guarded local learning tools to the clip studio.
Automatic processing and contribution sharing start off.

## Discovery and recording

- A factual ledger supplies exceptional-shot and possible-whiff candidates before
  visual review. Candidate windows are merged into bounded chronological batches.
  A separate full visual sweep remains available for moments without event triggers.
- Targeted previews use normal game time and measured frame/tick mapping. Higher
  demo playback speed is not enabled. Final clips retain their own recording settings.
- One real demo produced 5,581 ledger events and 1,894 candidates across ten players.
  Planned targeted footage was 5,949.5 seconds versus 21,873.3 seconds for the full
  sweep: 72.8% less footage. This is planning coverage, not a measured end-to-end
  speedup or a claim that every important moment was found.
- A new personal export passed native 3840 × 2160, 60 FPS, 602-frame/10.033-second
  capture validation, including measured tick mapping and the correct player POV.
  The owned CS2 process exited after recording. Existing 4K60 clips played inside
  the new native app, with measured media details and advancing playback time.
- Frozen source fingerprints, player/model/profile identities and export options
  prevent stale review results or retries from silently changing their source.
  Automatic overlapping attempts are deduplicated; manual exports remain available.
- A real v8 4B review passed all four windows of the same 10.033-second personal
  clip in 321.64 seconds, using unchanged 4 FPS sampling, 16/16/16/5 evidence frames,
  32K context and the existing confidence threshold. Correct negative responses
  no longer cause schema failures; short-window time/frame bounds are explicit.
  All four low-confidence proposals were excluded. Factual highlights remain
  independent. This establishes complete coverage of that event preview only,
  not whole-match coverage or subjective detection accuracy. The owned inference
  processes exited, and the saved default 8B model/settings remained unchanged.

## Optional contributions and learning

- Local feedback retains the original proposal and the user's correction. It does
  not upload footage or change Qwen weights by itself.
- Preparing, previewing, exporting an offline package and uploading are distinct
  actions. Selected clips omit audio by default and mask common HUD/name/chat areas;
  users still need to inspect their package. These masks do not prove anonymity.
- Raw demos may contain identifiers, chat and recorded voice. Speech processing
  has separate consent; declining speech does not remove it from a raw demo.
  The private receiver retains raw demos for 30 days.
- Interrupted uploads resume only from acknowledged offsets. Withdrawal is durable
  before the network request and blocks further transfer until deletion is resolved.
- The receiver stores opaque files without decoding, parsing, unzipping or training
  them. Service, transfer, storage and free-space limits are enforced separately.
- Actual local CPU validator tests passed with a rootless, network-disabled,
  read-only container: one CPU, 768 MiB RAM, 32 processes, no writable host mount,
  128 MiB per output and 256 MiB total output. Cancellation removed the owned
  container in 4.15 seconds while preserving an unrelated container. Timeout,
  malformed-media rejection and both actual output limits also passed.
- CPU preference calibration and Qwen LoRA training are separate workflows. Neural
  promotion requires independent grounded human labels, held-out contributors and
  matches, actual model reload and whole-match/export evidence. Rollback retains a
  prior accepted version. No newly trained neural model is bundled with this beta.

## Regression checks

| Area | Result |
| --- | --- |
| Core parser, discovery, rendering and storage | 368 passed; 13 intentionally ignored |
| Native application, paths and interfaces | 24 passed; 2 intentionally ignored |
| AI/evidence helpers | 32 passed; 1 optional real-sidecar test skipped |
| Steam helper | 36 passed; installed Valve-signed runtime and live match metadata verified |
| Contribution controller | 9 passed |
| Local learning controller | 21 passed; helper compilation passed |
| Receiver | 33 passed on Windows and 33 on Linux; 8 real HTTPS smoke checks passed |
| Frontend | Type checking, production build, community workflows and studio regressions passed |
| Community accessibility | Dark/light WCAG A/AA checks: zero violations in tested states |
| Release integrity | Valid signature/files accepted; tampered file, tampered manifest and signed traversal rejected |

The native smoke test restored the existing clip/match library and Steam connection.
The relocated public app used its own workspace and empty connection credentials.
Missing-helper capability checks keep download actions unavailable and give a visible
manual-import path on Home, Matches and Connection, including compact windows.
Public packaging tests cover relocated Python imports/status, fresh installation,
update, uninstall, paths containing spaces, busy-app refusal and linked/hard-linked
destination rejection. Updates and uninstall preserve user data. The previous 0.2.1
working package remains separate.

## Installation and integrity

Use the complete portable ZIP or installer. The portable package includes an isolated
Python helper and its required core libraries. Recording tools and optional models
are separate first-run downloads. The public beta excludes Valve's runtime and
the current Steam match-helper executable because its GPL/Steamworks linkage has
no verified redistribution exception. Fresh public installations must import demos
manually; automatic match-history download remains pending a cleared replacement.
The private development installation retains its existing working helper and
verified installed CS2 runtime.

Release assets include SHA-256 checksums, an Ed25519-signed manifest, its public key
and matching source. A manifest signature is not a Windows Authenticode certificate
and does not imply Microsoft SmartScreen reputation. The public key distributed
with the release provides a consistency check; trust its source independently.

## Current limits

- Whiffs, humor and other subjective detections need playtesting and independent
  labels. No whole-match accuracy improvement is claimed.
- The 8B model can be slow; preview reduction does not guarantee a particular ETA.
- A real 8B v7 review window validated the new multi-event response on the discrete
  GPU in 175.51 seconds, returning no event. The outer ten-minute whole-clip test
  stopped before all four windows completed. This is partial format compatibility
  evidence, not completed whole-clip coverage or an accuracy result.
- The fresh public package cannot download match history without a separately
  obtained compatible helper. Importing downloaded demos and recording clips work.
- Contributed raw demos remain in quarantine. Their isolated demo-to-example
  conversion worker is not enabled in this beta.
- GPU LoRA training has not passed the required backward/checkpoint pilot on the
  test PC. Storage headroom and the pilot must pass before training can run.
- Detailed visual grounding is an operator CLI workflow; a keep rating alone is
  insufficient for neural training.
- Generic manual AI-suggestion exports can retain a generic category; fact-based
  discovery exports retain their canonical candidate metadata.
- The recorded personal demos contained no voice packets, so actual voice-chat
  reconstruction was not validated. Game audio was recorded successfully.
- A shared host cannot promise zero impact from an external network flood, despite
  service quotas and isolation. Existing services passed before/after health checks.

Private test demos, gameplay, credentials, raw logs and local machine state are not
included in the public source or release. See [training](../training/README.md),
[receiver](../community_server/README.md) and [packaging](../packaging/public/README.md)
for the concrete boundaries and setup instructions.
