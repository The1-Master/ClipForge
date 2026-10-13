# ClipForge

A Windows clip studio for your Counter-Strike 2 demos. Find highlights, deaths,
exceptional shots, possible whiffs and unexpected moments; export selected
gameplay at your PC's recording quality. No subscription or clip-count quota.

## Install the beta

Download the installer or complete portable ZIP from
[The1-Master/ClipForge releases](https://github.com/The1-Master/ClipForge/releases).
Extract the whole portable folder before running ClipForge.exe. The installer
includes the native application and its isolated Python helper.

Installed CS2 and Microsoft WebView2 are required. Import a downloaded `.dem`
through **Import demo**, then open Settings > Tools to install the recording tools.
The public beta does not bundle the Steam match helper: its GPL/Steamworks linking
permissions need resolution before that binary can be distributed. Automatic
match-history download therefore remains unavailable in a fresh public installation.
The existing private development installation retains its working helper.
Local AI models are optional downloads in Settings > AI. Tools,
models, demos and exports require additional storage. Recording waits for your
ordinary CS2 session to close. Automatic processing starts **off**.

## Find more of your moments

The parser builds a factual event ledger first. Targeted review records candidate
windows at normal game time and gives AI measured frames around events. A
separate full visual sweep looks for surprises without an event trigger. Possible
whiffs remain suggestions until reviewed; headshots and other recorded events
are distinguished from subjective judgments. Faster demo timescale scouting is
not enabled without timing and coverage validation.

Selected exports default to native 4K60, with your chosen HUD and voice settings.
Actual capture resolution, frame rate and timing must pass validation. Voice is
available only when recorded in the demo. AI previews use a separate lightweight
profile. Hardware, available disk space and replay availability still matter.

## Help improve discovery

Use **Make the app better** on a moment, clip or replay. Original proposals and
your corrections stay together on your PC and inform future review preferences.
Feedback alone does not train new neural weights or upload footage.

Settings > Community & learning offers optional contributions. Preparing a
package, inspecting it and sending it are separate steps. You can export an
offline package, resume a transfer or request withdrawal. Raw demos are not
anonymous: they may contain player identifiers, chat and recorded voice. Speech
processing needs separate permission; declining it does not strip a raw demo.
Raw demos are retained for 30 days. Previously released model weights cannot be
promised to forget a withdrawn contribution instantly.

The private receiver stores opaque files under resource limits. Isolated local
workers validate media; independent human labels and held-out whole-match tests
gate model promotion. CPU preference learning and Qwen weight training are
separate. The beta does not include a newly trained Qwen model or a demonstrated
whole-match accuracy improvement. Contributed raw-demo conversion remains gated
until its isolated parser/render worker is available.

## Source, verification and licenses

ClipForge is an AGPL-3.0-only fork of CS DemoDesk by NOIH. Read [UPSTREAM.md](UPSTREAM.md),
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) and [LICENSE](LICENSE).
The original documentation is preserved in [README.upstream.md](README.upstream.md).
Qwen models retain their Apache-2.0 notices. Steam's Valve runtime and the current
linked Steam match-helper executable are not redistributed. The private helper
uses your verified installed CS2 runtime.
Optional recording and speech dependencies are obtained separately with their
own notices.

See [the beta verification notes](docs/BETA-0.3.0.md),
[public packaging](packaging/public/README.md), [private receiver](community_server/README.md)
and [learning worker](training/README.md) for tested behavior, setup and current limits.

Build the frontend with `npm ci` and `npm run build`; build the production desktop
app with `npm run tauri -- build --no-bundle`.
Public packaging scripts run relocation,
privacy and installer checks. This fork does not install upstream updates.
