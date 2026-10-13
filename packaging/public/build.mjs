import { copyFileSync, existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, realpathSync, statSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import os from 'node:os';

export const WORKSPACE = 'J:/CS2ClipStudio';
export const BASE_PYTHON = path.join(os.homedir(), '.cache/codex-runtimes/codex-primary-runtime/dependencies/python');
const BASE_FILES = ['python.exe', 'pythonw.exe', 'python3.dll', 'python312.dll', 'vcruntime140.dll', 'vcruntime140_1.dll', 'LICENSE.txt'];
export const PACKAGES = [
  ['anyio', 'anyio-4.15.1.dist-info'], ['certifi', 'certifi-2026.7.22.dist-info'],
  ['h11', 'h11-0.16.0.dist-info'], ['httpcore', 'httpcore-1.0.9.dist-info'],
  ['httpx', 'httpx-0.28.1.dist-info'], ['idna', 'idna-3.20.dist-info'],
  ['psutil', 'psutil-7.2.2.dist-info'], ['typing_extensions.py', 'typing_extensions-4.16.0.dist-info'],
  ['pip', 'pip-25.0.1.dist-info'],
];
const OMIT = new Set(['__pycache__', '.git', '.pytest_cache', 'sitecustomize.py', 'usercustomize.py']);
export function forbiddenRuntimePath(relative) {
  return relative.split(/[\\/]/).some(part => OMIT.has(part)) || /\.(?:pth|pyc|pyo)$/i.test(relative);
}
export function sha256(file) { return createHash('sha256').update(readFileSync(file)).digest('hex'); }
export function wheelRecords(source, distribution) {
  const entries = new Map();
  for (const line of readFileSync(path.join(source, distribution, 'RECORD'), 'utf8').split(/\r?\n/).filter(Boolean)) {
    const fields = []; let value = '', quoted = false;
    for (let index = 0; index < line.length; index++) {
      const ch = line[index];
      if (ch === '"') { if (quoted && line[index + 1] === '"') { value += '"'; index++; } else quoted = !quoted; }
      else if (ch === ',' && !quoted) { fields.push(value); value = ''; }
      else value += ch;
    }
    fields.push(value);
    if (quoted || fields.length !== 3 || entries.has(fields[0])) throw new Error('Invalid installed wheel record: ' + distribution);
    entries.set(fields[0], { hash: fields[1], size: fields[2] });
  }
  return entries;
}
export function wheelAllow(source, prefix, records) {
  return relative => {
    const item = path.join(source, prefix, relative);
    if (forbiddenRuntimePath(relative)) return false;
    if (statSync(item).isDirectory()) {
      const normalized = path.posix.join(prefix, relative.replaceAll('\\', '/')) + '/';
      return [...records].some(([file, record]) => file.startsWith(normalized) && record.hash.startsWith('sha256='));
    }
    const record = records.get(path.posix.join(prefix, relative.replaceAll('\\', '/')));
    if (!record || !record.hash) return false; // Never admit an unregistered/unverified local file.
    if (record.hash) {
      if (!record.hash.startsWith('sha256=') || record.hash.slice(7) !== createHash('sha256').update(readFileSync(item)).digest('base64url') || Number(record.size) !== statSync(item).size) throw new Error('Installed dependency changed from its wheel: ' + prefix);
    }
    return true;
  };
}
export function listFiles(root, relative = '') {
  const result = [];
  for (const item of readdirSync(path.join(root, relative), { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    const child = path.join(relative, item.name);
    if (item.isSymbolicLink()) throw new Error(`Package inputs cannot contain links: ${child}`);
    if (item.isDirectory()) result.push(...listFiles(root, child));
    else if (item.isFile()) result.push(child.replaceAll('\\', '/'));
  }
  return result;
}
export function copyTree(source, destination, allow = () => true, relative = '') {
  if (!existsSync(source)) throw new Error('Required package input missing: ' + path.basename(source));
  if (lstatSync(source).isSymbolicLink()) throw new Error('Package source cannot be a symlink');
  if (!allow(relative)) return;
  if (statSync(source).isDirectory()) {
    mkdirSync(destination, { recursive: true });
    for (const item of readdirSync(source)) copyTree(path.join(source, item), path.join(destination, item), allow, path.join(relative, item));
  } else {
    mkdirSync(path.dirname(destination), { recursive: true });
    copyFileSync(source, destination);
  }
}
export function assertOwnedOutput(output) {
  const resolved = path.resolve(output);
  const authorized = path.resolve(WORKSPACE, 'artifacts');
  if (!resolved.startsWith(authorized + path.sep) || resolved === authorized) throw new Error('Package output must stay beneath the authorized artifact directory');
  let ancestor = resolved;
  while (!existsSync(ancestor)) ancestor = path.dirname(ancestor);
  const canonicalArtifacts = realpathSync(authorized).toLowerCase();
  const canonicalAncestor = realpathSync(ancestor).toLowerCase();
  if (lstatSync(ancestor).isSymbolicLink() || (canonicalAncestor !== canonicalArtifacts && !canonicalAncestor.startsWith(canonicalArtifacts + path.sep))) throw new Error('Package output cannot traverse a link');
  return resolved;
}
function safeBoilerProvenance(input) {
  return {
    schemaVersion: 1, repository: input.repository, version: input.version, commit: input.commit,
    annotatedTagObject: input.annotatedTagObject, sourceLicense: input.sourceLicense,
    npmMetadataLicense: input.npmMetadataLicense,
    licenseDiscrepancy: 'Top-level MIT conflicts with package GPL-3.0 metadata and retained GPL-3.0-or-later Boiler source headers. Treat inherited files as GPL; no Steamworks linking exception found.',
    excludedUpstreamExecutableSHA256: input.files.find(f => f.name === 'boiler-writter.exe').sha256,
    executableBundled: false, sourceOnly: true,
    sourceComponentLicense: 'GPL-3.0-or-later headers in CSGOClient.cpp, CSGOClient.h and GCMsgHandler.h; top-level MIT retained for other upstream contributions',
    sourceCompleteness: 'Pinned helper source and required protobuf 3.20.3 source are included; Valve SDK is excluded. No distribution claim is made for a Steamworks-linked executable.',
    linkingExceptionFound: false,
    executableUnsigned: true, reproducibleBuildVerified: false,
    valveRuntimeBundled: false,
    valveRuntimePolicy: 'Never ship Valve SDK headers, import libraries or runtime DLLs. Steam match downloads require a separately supplied, license-cleared helper; private-use source build prerequisites do not grant redistribution rights.',
  };
}
function noticeInputs(repo) {
  return [
    ['LICENSE', 'ClipForge-AGPL-3.0.txt'], ['vendor/demoparser/LICENSE', 'demoparser-MIT.txt'],
    ['public/fonts/Inter-LICENSE.txt', 'Inter-4.1-OFL.txt'], ['assets/branding/Bootstrap-Icons-LICENSE.txt', 'Bootstrap-Icons-MIT.txt'],
    ['crates/demodesk-core/window-hook/Detours.LICENSE.md', 'Detours-MIT.txt'],
    ['crates/demodesk-core/src/render/CSDM.LICENSE.txt', 'CS-Demo-Manager-MIT.txt'],
    ['crates/demodesk-core/src/smoke/LICENSE', 'smoke-GPL-3.0.txt'],
    ['public/weapons/LICENSE', 'Counter-Strike-assets-and-tooling.txt'],
  ].map(([file, name]) => [path.join(repo, file), name]);
}
export function buildPublic({ repo, exe, output, expectedExeSha, basePython = BASE_PYTHON, packageSource = path.join(WORKSPACE, 'tools/python-env/Lib/site-packages'), boiler = path.join(WORKSPACE, 'tools/boiler'), boilerSource = path.join(WORKSPACE, 'research/upstream/boiler-writter') }) {
  const version = JSON.parse(readFileSync(path.join(repo, 'package.json'), 'utf8')).version;
  const versionProbe = spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', path.join(repo, 'packaging/public/probe-version.ps1'), '-Executable', exe], { encoding: 'utf8', windowsHide: true, timeout: 30000, env: { ...process.env, TEMP: path.join(WORKSPACE, 'temp'), TMP: path.join(WORKSPACE, 'temp') } });
  if (versionProbe.status !== 0 || JSON.parse(versionProbe.stdout).productVersion !== version) throw new Error('Native executable version does not match this public package');
  const executableSha = sha256(exe);
  if (expectedExeSha !== undefined && (!/^[a-f0-9]{64}$/.test(expectedExeSha) || expectedExeSha !== executableSha)) throw new Error('Native executable differs from the accepted build hash');
  output = assertOwnedOutput(output ?? path.join(WORKSPACE, 'artifacts', `ClipForge-${version}-public`, 'ClipForge'));
  mkdirSync(output, { recursive: true });
  // Rebuild in a clean owned staging folder; never merge private/user data into a distributable.
  if (readdirSync(output).length) throw new Error('Public staging folder must be empty; use a new owned staging directory');
  copyTree(exe, path.join(output, 'ClipForge.exe'));
  if (sha256(path.join(output, 'ClipForge.exe')) !== executableSha) throw new Error('Native executable changed while packaging');
  const python = path.join(output, 'tools/python');
  for (const file of BASE_FILES) copyTree(path.join(basePython, file), path.join(python, file));
  copyTree(path.join(basePython, 'DLLs'), path.join(python, 'DLLs'), rel => !forbiddenRuntimePath(rel) && !/(^|[\\/])_test|_ctypes_test/i.test(rel));
  copyTree(path.join(basePython, 'Lib'), path.join(python, 'Lib'), rel => !forbiddenRuntimePath(rel) && !rel.split(/[\\/]/).some(part => ['site-packages', 'test', 'tests', 'idlelib', 'turtledemo'].includes(part)));
  copyTree(path.join(basePython, 'tcl'), path.join(python, 'tcl'), rel => !forbiddenRuntimePath(rel));
  const dependencyInventory = [];
  for (const [module, distribution] of PACKAGES) {
    const records = wheelRecords(packageSource, distribution);
    const admitModule = wheelAllow(packageSource, module, records), admitMetadata = wheelAllow(packageSource, distribution, records);
    copyTree(path.join(packageSource, module), path.join(python, 'Lib/site-packages', module), rel => admitModule(rel) && !rel.split(/[\\/]/).some(part => ['tests', 'test'].includes(part)));
    copyTree(path.join(packageSource, distribution), path.join(python, 'Lib/site-packages', distribution), rel => admitMetadata(rel) && !/(^|[\\/])(direct_url\.json|INSTALLER|REQUESTED|RECORD)$/.test(rel));
    const metadata = readFileSync(path.join(packageSource, distribution, 'METADATA'), 'utf8');
    const licenseFiles = listFiles(path.join(python, 'Lib/site-packages', distribution)).filter(file => /LICENSE|COPYING|NOTICE/i.test(file));
    if (!licenseFiles.length) throw new Error('Dependency has no retained license: ' + distribution);
    dependencyInventory.push({ module, distribution, name: /^Name: (.+)$/m.exec(metadata)?.[1], version: /^Version: (.+)$/m.exec(metadata)?.[1], licenseFiles });
    // Supply an accurate RECORD for this curated layout, so optional pip installs
    // can upgrade a core dependency without inheriting venv Scripts paths.
    const site = path.join(python, 'Lib/site-packages');
    const installed = [...(statSync(path.join(site, module)).isDirectory() ? listFiles(path.join(site, module)).map(file => module + '/' + file) : [module]), ...listFiles(path.join(site, distribution)).map(file => distribution + '/' + file)];
    const rows = installed.map(file => `${file},sha256=${createHash('sha256').update(readFileSync(path.join(site, file))).digest('base64url')},${statSync(path.join(site, file)).size}`);
    rows.push(`${distribution}/RECORD,,`);
    writeFileSync(path.join(site, distribution, 'RECORD'), rows.join('\n') + '\n');
  }
  copyTree(path.join(repo, 'automation'), path.join(output, 'app/automation'), rel => !forbiddenRuntimePath(rel) && !/(^|[\\/])(?:test_|.*_tests\.py)/.test(rel) && (rel === '' || statSync(path.join(repo, 'automation', rel)).isDirectory() || /\.(py|txt|md|json)$/.test(rel)));
  copyTree(path.join(repo, 'training'), path.join(output, 'app/training'), rel => !forbiddenRuntimePath(rel) && !/(^|[\\/])(?:test_|.*_tests\.py)/.test(rel) && (rel === '' || statSync(path.join(repo, 'training', rel)).isDirectory() || /\.(py|txt|md|json|yml|yaml)$/.test(rel) || /(^|[\\/])Containerfile(?:\.[A-Za-z0-9_-]+)?$/.test(rel)));
  const executableProvenance = JSON.parse(readFileSync(path.join(boiler, 'PROVENANCE.json'), 'utf8'));
  // The inherited GPL source has no Steamworks linking exception. Keep its
  // public source/notices, but never redistribute the prebuilt linked helper.
  copyTree(path.join(boiler, 'LICENSE-MIT.txt'), path.join(output, 'tools/boiler/LICENSE-MIT.txt'));
  copyTree(path.join(boiler, 'PACKAGE-METADATA.json'), path.join(output, 'tools/boiler/PACKAGE-METADATA.json'));
  const pinned = spawnSync('git', ['-C', boilerSource, 'rev-parse', 'HEAD'], { encoding: 'utf8' });
  if (pinned.status !== 0 || pinned.stdout.trim() !== executableProvenance.commit) throw new Error('Steam helper source does not match pinned commit');
  copyTree(boilerSource, path.join(output, 'sources/boiler-writter-1.7.0'), rel => !forbiddenRuntimePath(rel) && !/(^|[\\/])(?:bin|dist|build|steamworks(?:_sdk)?|node_modules)(?:[\\/]|$)/i.test(rel) && !/\.(?:dll|exe|lib|pdb)$/i.test(rel));
  const protobufSource = path.join(repo, 'packaging/public/sources/protobuf-cpp-3.20.3.zip');
  if (sha256(protobufSource) !== '2c65db747b0fec51e84480b7092aaff88b072fb27f9187073a219e09bb748bdf') throw new Error('Pinned protobuf source archive changed');
  const sourceCheck = [
    'import pathlib,sys,zipfile,stat',
    'with zipfile.ZipFile(sys.argv[1]) as z:',
    ' infos=z.infolist(); assert len(infos)<10000 and sum(i.file_size for i in infos)<100*1024*1024',
    ' for i in infos:',
    '  p=pathlib.PurePosixPath(i.filename); assert not p.is_absolute() and ".." not in p.parts and "\\\\" not in i.filename',
    '  assert not stat.S_ISLNK(i.external_attr>>16) and p.suffix.lower() not in (".dll",".exe",".lib",".pdb")',
    ' assert "#define GOOGLE_PROTOBUF_VERSION 3020003" in z.read("protobuf-3.20.3/src/google/protobuf/stubs/common.h").decode()',
    ' assert z.read("protobuf-3.20.3/LICENSE").decode().strip()==pathlib.Path(sys.argv[2]).read_text().strip()',
  ].join('\n');
  const checkedSource = spawnSync(path.join(basePython, 'python.exe'), ['-I', '-B', '-c', sourceCheck, protobufSource, path.join(repo, 'packaging/public/licenses/protobuf-3.20.3-BSD-3-Clause.txt')], { encoding: 'utf8', windowsHide: true, timeout: 30000, env: { ...process.env, PYTHONHOME: '', PYTHONPATH: '', TEMP: path.join(WORKSPACE, 'temp'), TMP: path.join(WORKSPACE, 'temp') } });
  if (checkedSource.status !== 0) throw new Error('Protobuf source completeness/safety/license check failed: ' + checkedSource.stderr);
  copyTree(protobufSource, path.join(output, 'sources/protobuf-cpp-3.20.3.zip'));
  copyTree(path.join(repo, 'packaging/public/LEGAL-AUDIT.md'), path.join(output, 'sources/LEGAL-AUDIT.md'));
  const licenses = path.join(output, 'licenses');
  for (const [file, name] of noticeInputs(repo)) copyTree(file, path.join(licenses, name));
  copyTree(path.join(WORKSPACE, 'tools/nsis/nsis-3.12/COPYING'), path.join(licenses, 'NSIS-3.12-COPYING.txt'));
  copyTree(path.join(repo, 'packaging/public/licenses'), path.join(licenses, 'packaged-dependencies'), rel => !forbiddenRuntimePath(rel));
  copyTree(path.join(repo, 'LICENSE'), path.join(output, 'LICENSE.txt'));
  copyTree(path.join(repo, 'LICENSE'), path.join(output, 'app/LICENSE'));
  writeFileSync(path.join(output, 'tools/boiler/PROVENANCE.json'), JSON.stringify(safeBoilerProvenance(executableProvenance), null, 2) + '\n');
  const upstream = 'ClipForge is an AGPL-3.0-only fork of CS DemoDesk v1.0.29 by NOIH, upstream commit a85e85130d80d72a3415ee8ee01ee94626c43e5f.\nOriginal source: https://github.com/noih/cs-demodesk\nClipForge source destination: https://github.com/The1-Master/ClipForge\nKeep the exact corresponding-source archive beside this distribution.\n';
  writeFileSync(path.join(output, 'UPSTREAM.md'), upstream);
  writeFileSync(path.join(output, 'app/UPSTREAM.md'), upstream);
  const notices = [
    '# ClipForge public package notices', '',
    'ClipForge/CS DemoDesk: AGPL-3.0-only. Original NOIH attribution is retained. Native corresponding source must accompany any public release.',
    'CPython 3.12: PSF license and included third-party notices at tools/python/LICENSE.txt; bundled pure Python dependencies retain their licenses in their .dist-info directories.',
    'CPython native modules: OpenSSL 3.5.8 Apache-2.0, libffi MIT, Expat 2.8.3 MIT, zlib 1.3.2 and mpdecimal 2.5.1 BSD notices at licenses/packaged-dependencies; liblzma public-domain family notice is retained without claiming an unverified exact version. Tcl/Tk license terms remain under tools/python/tcl. SQLite is public domain (https://www.sqlite.org/copyright.html). Microsoft runtime terms and bzip2 notice are included in tools/python/LICENSE.txt.',
    'Python packaging changes: interpreter/native binaries are unchanged; base site-packages, tests, caches and startup customizations were excluded, and nine explicitly verified core application packages were added. No source-to-binary reproducibility claim is made.',
    'psutil includes its own BSD-licensed Windows extension; its matching package metadata, Python wrappers, license and version are retained.',
    'certifi: MPL-2.0 certificate data/source is included unchanged in tools/python/Lib/site-packages/certifi.',
    'Core application notices: licenses/ contains demoparser, Inter, Bootstrap Icons, Detours, CS Demo Manager attribution and smoke GPL text.',
    'Counter-Strike icon assets belong to Valve and have separate terms; see licenses/Counter-Strike-assets-and-tooling.txt.',
    'Installer: NSIS 3.12 with its zlib compression module and standard plugins. Applicable notices are retained at licenses/NSIS-3.12-COPYING.txt; no LZMA or bzip2 compression module is used.',
    'boiler-writter 1.7.0 SOURCE ONLY: top-level LICENSE is MIT; CSGOClient.cpp/.h and GCMsgHandler.h retain Ansas Bogdan GPL-3.0-or-later grants. GPL-3.0 package metadata, copyright headers, GPL text and upstream helper source are retained. The prebuilt helper EXE is EXCLUDED because no Steamworks linking exception was found.',
    'protobuf 3.20.3: complete C++ source archive and BSD-3-Clause license are retained under sources/ and licenses/packaged-dependencies. This is the version required by the pinned helper workflow; a matching binary reproduction has not been verified.',
    'Valve SDK headers, import libraries and steam_api64.dll are EXCLUDED. Public-package Steam match downloads require a separately supplied, license-cleared helper. Demo import, discovery, recording and AI workflows remain available with their documented tool setup.',
    'HLAE, FFmpeg, Source 2 Viewer and Ollama/models are NOT bundled. Install the required recording tools in Settings > Tools; AI runtime/models in Settings > AI. Their downloaded notices remain beside them.',
    'Optional speech dependencies are installed locally on request. No user models, recordings, credentials, databases or browser profiles are distributed.', '',
  ].join('\n');
  writeFileSync(path.join(output, 'THIRD-PARTY-NOTICES.md'), notices);
  writeFileSync(path.join(output, 'app/THIRD-PARTY-NOTICES.md'), notices);
  writeFileSync(path.join(output, 'clipforge-portable.json'), JSON.stringify({ schemaVersion: 1, product: 'ClipForge', version, executable: 'ClipForge.exe', python: 'tools/python/python.exe', workspace: '.', autoProcessing: false }, null, 2) + '\n');
  writeFileSync(path.join(output, 'START-HERE.txt'), [
    `ClipForge ${version} — Windows public-beta package`, '',
    'Run ClipForge.exe from this complete folder. Keep app, tools, licenses and sources beside it.',
    'You can move the complete folder to another writable location. Its adjacent folder is the owned workspace.',
    'For the installer, choose a private writable folder outside AppData and system folders. Updates preserve user data and downloaded helpers.',
    'An installed Steam client, CS2 and Microsoft WebView2 Runtime are external prerequisites.',
    'First launch: Settings > Tools > install recording tools (HLAE and FFmpeg); install Source 2 Viewer there for replay assets.',
    'Optional AI: Settings > AI > install the local vision runtime/models. Model downloads require substantial disk space.',
    'Import your own downloaded demo to begin. Automatic processing starts OFF.',
    'Steam match download is unavailable in this public candidate until a separately supplied helper has a clear redistribution/license basis. The private development build retains its existing helper.',
    'Raw demo contributions are optional. Preview the contents/consent before sending. Steam credentials remain private.',
    'Private operator learning scripts are included under app/training. They require separate accepted isolation/hardware setup; no trainer, WSL image or neural weights are installed automatically.',
    'No personal demos, clips, models, user settings, token credentials or browser profiles are in this package.',
    'Recording controls only ClipForge-owned CS2 processes and waits while your own game is open.',
    'Voice chat can appear only when it was recorded in the demo. Reviewed AI suggestions are experimental.',
    'Uninstall removes packaged application files; it preserves data, state, models, recordings and downloaded helper files.',
    'Read LICENSE.txt, THIRD-PARTY-NOTICES.md, UPSTREAM.md and PUBLICATION-CHECKLIST.txt.', '',
  ].join('\r\n'));
  writeFileSync(path.join(output, 'PUBLICATION-CHECKLIST.txt'), [
    'This is a locally prepared distribution candidate. No public publication is performed by packaging.',
    'Before public publication, attach the complete matching ClipForge native source archive and these notices.',
    'Do not add the Steam helper EXE: inherited GPL-3.0-or-later source has no observed Steamworks linking exception. Retain its source, GPL/MIT notices and protobuf source; a public replacement requires its own verified licensing/build basis.',
    'Do not add Valve Steamworks runtime DLLs, models, recordings, user profiles or credentials to the package.',
    'Recording and AI tools remain explicit first-run downloads with their own source/license requirements.',
    'The native executable and installer are not Authenticode signed unless a separate signing step is performed.', '',
  ].join('\r\n'));
  writeFileSync(path.join(output, 'DEPENDENCIES.json'), JSON.stringify({ schemaVersion: 1, python: '3.12', core: dependencyInventory, excludedOptional: ['faster-whisper', 'av', 'ctranslate2', 'onnxruntime', 'numpy', 'pandas', 'demoparser2'], firstRunDownloads: ['HLAE', 'FFmpeg', 'Source 2 Viewer', 'Ollama', 'Qwen3-VL', 'Whisper Small'] }, null, 2) + '\n');
  const files = listFiles(output).map(file => ({ path: file, bytes: statSync(path.join(output, file)).size, sha256: sha256(path.join(output, file)) }));
  writeFileSync(path.join(output, 'PACKAGE-MANIFEST.json'), JSON.stringify({ schemaVersion: 1, product: 'ClipForge', version, createdAt: new Date().toISOString(), packageType: 'complete-relocatable-folder-with-first-run-tools', files }, null, 2) + '\n');
  return { output, version, files: files.length, bytes: files.reduce((sum, file) => sum + file.bytes, 0) };
}
