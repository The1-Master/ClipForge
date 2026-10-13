# Relocatable Windows distribution

This produces a complete core folder, ZIP and per-user NSIS installer. The public
folder includes the native app, standalone CPython, required core dependencies,
automation and private-operator training source, pinned helper source, full protobuf
C++ source and licenses. It
excludes personal data, models, credentials, browser profiles, Valve runtime DLLs
and the Steamworks-linked helper executable, as well as optional media packages.
Public-candidate Steam match downloads await a separately supplied license-cleared
helper; demo import remains available. Recording/AI tools are explicit first-run
downloads through Settings; installed Steam/CS2/WebView2 remain prerequisites.

Produce the native executable with the production Tauri build, which embeds the
frontend and enables its production protocol:

    npm run tauri -- build --no-bundle

Build the distribution from the application directory **after that exact native
executable passes its checks and an actual fresh-workspace GUI launch without a
development server**. Read-only version probes and moved Python CLI checks do
not establish that the native WebView can load its production frontend.

```powershell
node scripts/portable.mjs target/release/demodesk.exe --public --replace-staging --expected-exe-sha <accepted-native-SHA256>
node scripts/test-portable.mjs
node packaging/public/archive.mjs 'J:\CS2ClipStudio\artifacts\ClipForge-0.3.0-public\ClipForge'
node scripts/test-portable-installer.mjs
```

`--replace-staging` validates every old package hash and rejects unregistered
runtime/user files before moving the prior candidate to a separate artifact
folder. It never deletes or overwrites user state. The private executable package
continues to use the separate `ClipForge-<version>` artifact directory; older
0.1.0/0.2.0/0.2.1 packages are untouched.

The base runtime is read only. An explicit file/directory policy excludes its
existing site-packages, environment metadata, startup customizations and .pth
files. Only the allowlisted application packages are admitted. Their licenses
are checked, their distribution records describe the curated layout, and the
result is tested after relocation to a folder containing spaces with isolated
workspace/cache/temp variables. No user-profile state is written by tests.

NSIS 3.12 is downloaded from the primary official SourceForge project into the
authorized tools folder. Archive SHA-256 verified against its primary download
page: `56581f90db321581c5381193d796fffcf2d24b2f8fed2160a6c6a3baa67f2c4f`.
Compiler path: `J:/CS2ClipStudio/tools/nsis/nsis-3.12/Bin/makensis.exe`.
The compiler distribution retains its licenses in that tools folder.

The installer requires an empty writable folder or an existing marked ClipForge
workspace, rejects AppData/system roots and linked ancestors, and preserves
runtime data and additional downloaded helpers across updates. Every registered
destination is checked for internal reparse points and hard links before changes;
an open/locked application blocks update/uninstall without terminating it. Uninstall deletes
only exact packaged files; it preserves user data, extra tools and the ownership
marker so reinstallation can reuse the workspace. `/FIXTURE` suppresses registry
and Start-menu actions for controlled tests; it does not relax path validation.

Deliverables live in `J:/CS2ClipStudio/artifacts/ClipForge-0.3.0-public`:
the `ClipForge` folder, Windows ZIP, Setup EXE, per-file manifest, checksums and
verification reports. Do not publish the private test/history folders.

Before public publication, attach the matching complete modified application
source archive and preserve all notices. boiler-writter contains actual inherited
GPL-3.0-or-later source despite its top-level MIT file. No Steamworks linking
exception was found, so its executable, SDK headers/import libraries and Valve
runtime DLLs are excluded. The original GPL headers, helper source/build scripts,
GPL text and complete required protobuf source are retained. Read
[LEGAL-AUDIT.md](LEGAL-AUDIT.md) for evidence and the precise source-completeness
limits; this source-only inclusion is not claimed as complete corresponding
source for an excluded proprietary-linked executable.
Optional FFmpeg/HLAE/speech binaries are omitted until their redistribution/source
obligations are satisfied. This build does not publish, upload or sign anything.
