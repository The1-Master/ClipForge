# Public package source and license audit

Checked 2026-10-13. This records concrete source/build evidence and the package
policy; it does not assign one blanket license to every bundled component.

## Steam match helper

The original helper is boiler-writter 1.7.0, upstream commit
fafb37c1b9bc53e7adff9c4f9ab3b5da68064da0. Its top-level LICENSE and README
say MIT (AkiVer, 2016-present), and package.json says GPL-3.0. More significantly,
boiler-writter/CSGOClient.cpp, CSGOClient.h and GCMsgHandler.h retain Ansas
Bogdan's 2015 Boiler copyright and GPL-3.0-or-later grants. The package retains
all those original notices and GPL v3 text. The MIT top-level file is not used
as permission to erase or relicense the inherited GPL contributions.

The pinned CMake file requires Steamworks, includes its SDK headers and links
its import library. The pinned release workflow downloads Steamworks SDK 1.63
and protobuf 3.20.3. No Steamworks linking exception or additional permission
grant appears in the inspected helper LICENSE, README, source headers or build
workflow. Copying a Valve-signed runtime from a user's own installation proves
its signature; it does not establish permission to distribute a GPL-linked
helper or supply excluded SDK source.

Therefore the public package contains **source only**: the original helper
checkout, build scripts and notices, plus the complete official protobuf C++
3.20.3 source archive and BSD-3-Clause notice. It excludes the helper executable,
Valve SDK headers, import libraries and Valve runtime DLLs. Steam match download
in this public candidate requires a separately supplied helper with a verified
licensing/build basis. Importing an existing demo remains supported. The private
development installation is preserved.

This is the full pinned helper project source, not a claim that it is complete
corresponding source for the excluded upstream executable. It excludes Valve's
SDK and no source-to-binary reproduction or upstream binary attestation has been
verified. Protobuf source is the version required by the pinned workflow, not an
independently proved reconstruction of its executable.

For any future GPL helper executable distribution, provide its matching source,
linked non-system components and build/install scripts as required by the GPL,
retain all copyright/license notices, and establish permission for any required
proprietary SDK/linkage. Do not substitute an ordinary repository URL for the
actual source needed to build that binary. No System Library exception for
Steamworks is asserted here.

An advanced private-use build can follow the pinned README after the user
obtains and accepts their own SDK, with Visual Studio 2022 C++ build tools,
CMake and protobuf 3.20.3. The package never downloads/accepts SDK terms or
redistributes SDK material automatically. This path has not been exercised as
a clean first-run build and is not presented as an ordinary user's setup flow
or a solution to binary redistribution.

Evidence:

- [Pinned GPL source header](https://github.com/akiver/boiler-writter/blob/fafb37c1b9bc53e7adff9c4f9ab3b5da68064da0/boiler-writter/CSGOClient.cpp)
- [Pinned build/link settings](https://github.com/akiver/boiler-writter/blob/fafb37c1b9bc53e7adff9c4f9ab3b5da68064da0/CMakeLists.txt)
- [Pinned release dependency workflow](https://github.com/akiver/boiler-writter/blob/fafb37c1b9bc53e7adff9c4f9ab3b5da68064da0/.github/workflows/release.yml)
- [GPL v3, source definitions and distribution conditions](https://www.gnu.org/licenses/gpl-3.0.html)
- [Official protobuf source asset](https://github.com/protocolbuffers/protobuf/releases/download/v3.20.3/protobuf-cpp-3.20.3.zip)

The source asset is 5,895,322 bytes, SHA-256
2c65db747b0fec51e84480b7092aaff88b072fb27f9187073a219e09bb748bdf.
This is a locally computed integrity pin after HTTPS download from the official
release; the legacy release API supplied no independent published SHA digest.
Its source entries, version macro and LICENSE are checked during packaging.

## CPython and native dependency provenance

The package copies an unchanged CPython 3.12.14 runtime from the configured
read-only desktop dependency distribution. It excludes preexisting
site-packages, tests, caches, venv metadata, .pth files and startup
customizations, then adds nine explicitly allowlisted wheel-verified core
application packages. PSF and original Microsoft runtime restrictions,
bzip2 and Tcl/Tk terms are retained. This summary also records the repackaging
changes required by the PSF terms.

The relocated runtime reports OpenSSL 3.5.8, Expat 2.8.3, zlib 1.3.2,
mpdecimal 2.5.1 and SQLite 3.53.1. Their Apache-2.0, MIT, zlib, BSD-2-Clause
and public-domain notices are preserved. OpenSSL's notice comes from its exact
reported tag; mpdecimal's notice is the original header from CPython's pinned
source. liblzma's public-domain family notice is retained; its exact build
version was not established and no unrelated xz command-line programs ship.

libffi-8.dll has no PE version/copyright metadata. Its shipped SHA-256 is
f39e5a9ff36f4c3dd74b72bce4726e07b8b77ceec92d4a8d5b788e7f0bc061af.
It does **not** match CPython's official cpython-bin-deps/libffi-3.4.4
amd64 artifact (SHA-256
eff52743773eb550fcc6ce3efc37c85724502233b6b002a35496d828bd7b280a).
CPython's version alone therefore cannot identify this separately packaged
dependency's exact source/build. The package retains the upstream libffi MIT
copyright and permission notice from v3.4.6 as a documented license-family
notice, without asserting that this DLL was built from that tag. Exact libffi
binary-source provenance and reproducible-build evidence remain unresolved;
no known GPL library or source-offer obligation is asserted for libffi.

See licenses/packaged-dependencies/PROVENANCE.json for exact notice URLs,
observed versions and hashes. The permissive notices do not turn this runtime
into a reproducibly built or universally relicensed artifact.

## Application publication

ClipForge is AGPL-3.0-only and contains components with their own compatible
license conditions. A binary release must provide the matching complete
modified application source, including relevant vendored source, lockfiles,
build/configuration scripts and retained dependency notices. This audit does not
replace that release source archive. Preserve the separate Valve asset notice;
the application license does not grant rights to game assets.

FFmpeg, HLAE, Source 2 Viewer, Ollama and models are not bundled here; user-requested
downloads retain their own source/license conditions. No signing or publication
is performed by these packaging scripts.
