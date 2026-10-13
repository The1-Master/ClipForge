# ClipForge dependency and distribution notices

The development workspace retains upstream source, package licenses and runtime notices. This index identifies important locations; it does not replace their license text or declare every tool, model and game asset to have the application's license. Paths beginning with `tools/`, `data/` or `models/` below describe private or separately obtained dependencies, not files promised inside the public source archive. Application paths beginning with `app/` are relative to the development workspace; remove that prefix in the public source checkout.

The public package carries an isolated CPython/core-library runtime and its actual license files under `licenses/`, with per-file hashes and dependency provenance. Its open-source helper/protobuf material is source only. The linked Steam helper executable, Valve SDK headers/runtime DLL, FFmpeg/HLAE/speech binaries, AI weights and private data are excluded. Recording and model tools are separate downloads. See [packaging/public/README.md](packaging/public/README.md) and [docs/BETA-0.3.0.md](docs/BETA-0.3.0.md).

| Component | Notice retained in this workspace |
| --- | --- |
| CS DemoDesk / ClipForge | `app/LICENSE` (AGPL-3.0-only), `app/UPSTREAM.md`; upstream NOIH attribution |
| Vendored demoparser | `app/vendor/demoparser/LICENSE` (MIT) |
| Inter 4.1 | `app/public/fonts/Inter-LICENSE.txt` (SIL Open Font License 1.1); official release hashes and font provenance in `Inter-provenance.json` |
| Bootstrap Icons 1.13.1 SVGs | `app/assets/branding/Bootstrap-Icons-LICENSE.txt` (MIT), `icon-provenance.json`; locally bundled paths used by the studio icon component |
| axe-core / Playwright accessibility checks | Development-only test tooling; original MPL-2.0 and MIT notices retained in `app/node_modules` |
| Microsoft Detours window helper | `app/crates/demodesk-core/window-hook/Detours.LICENSE.md` (MIT) |
| CS Demo Manager video-profile/recording approach | `app/crates/demodesk-core/src/render/CSDM.LICENSE.txt` (MIT); pinned source `b45a5f29283590c3fd0fc6526564b49567c0b931`, complete profile and ordinary-windowed recording launch referenced by `video.rs` and `record.rs` |
| Source 2 Viewer CLI 20.0 | `data/clipforge/tools/vrf/LICENSE-MIT.txt`; signed binary/source/package hashes retained in `PROVENANCE.json` beside the tool and `research/benchmarks/desktop/source2viewer-provenance.json` |
| Smoke implementation | `app/crates/demodesk-core/src/smoke/LICENSE` (GPL v3 text) |
| FidelityFX CAS adaptation | Original AMD copyright/permission notice at the beginning of `app/src/replay/cas.ts` |
| Counter-Strike icon assets | `app/public/weapons/LICENSE`: Valve-owned assets have separate terms; its code/tooling permission does not license the assets |
| HLAE 2.192.7 | `tools/hlae/readme.txt`, `tools/hlae/LICENSES`; exact pinned source MIT notice restored at `LICENSES/advancedfx/LICENSE`, existing `THIRDPARTY.yml` retained |
| Installed FFmpeg GPL build | `tools/ffmpeg/ffmpeg-master-latest-win64-gpl/LICENSE.txt`; archive/source/configuration provenance in `research/benchmarks/render/tool-provenance.json` |
| boiler-writter 1.7.0 | Pinned top-level MIT text is retained, but `CSGOClient.cpp`, `CSGOClient.h` and `GCMsgHandler.h` contain Ansas Bogdan GPL-3.0-or-later notices. Those notices govern that code and are not erased by the MIT label. Compatible source and protobuf 3.20.3 source/licenses accompany the public distribution; the linked helper EXE is excluded. |
| Valve Steamworks runtime | `tools/boiler/bin/steam_api64.dll` is a separately licensed private dependency. The current helper includes Valve SDK headers and links `steam_api64`; no GPL linking exception was found in its pinned source or build instructions. No Valve SDK headers, DLL or linked helper executable are redistributed. |
| Ollama and bundled inference libraries | `tools/ollama/v0.40.3/lib/ollama/*LICENSE*` and `*NOTICE*`; runtime archive/hash provenance in model setup source/manifests |
| Qwen3-VL vision models | `models/licenses/qwen3-vl-8b-instruct.txt`, `qwen3-vl-4b-instruct.txt`, model manifests |
| Faster Whisper / speech model | Python package notices under `tools/python-env/Lib/site-packages`; downloaded model card/files under `models/huggingface`, manifest `models/whisper-small.json` |
| JavaScript/Rust/Python dependencies | Original notices remain under `app/node_modules`, `tools/rust/cargo/registry/src`, and `tools/python-env/Lib/site-packages`; lockfiles pin application dependencies |

Source 2 Viewer is attributed to ValveResourceFormat under MIT in the About dialog. CLI 20.0 is installed and verified for local replay assets. Steam, CS2 and WebView2 are external runtimes with their own terms.

In the private development workspace, the boiler-writter binary and Valve DLL match the official npm 1.7.0 package byte for byte. SHA-512 package integrity and SHA-1 registry digest were checked, and Valve's DLL Authenticode signature reported valid. This provenance check is not a redistribution permission. The helper executable is unsigned; npm attestation/signature verification and a source-to-binary reproducible build were not performed.

Keep all original notices and complete modified source for redistribution. The public source checkout preserves upstream provenance, embedded notices and build instructions. The beta's source and release assets belong to The1-Master/ClipForge.
