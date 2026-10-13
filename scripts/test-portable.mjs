import assert from 'node:assert/strict';
import { cpSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { assertOwnedOutput, buildPublic, copyTree, forbiddenRuntimePath, listFiles, sha256, wheelAllow, wheelRecords } from '../packaging/public/build.mjs';

const folder = assertOwnedOutput(process.argv[2] ?? 'J:/CS2ClipStudio/artifacts/ClipForge-0.3.0-public/ClipForge');
const manifest = JSON.parse(readFileSync(path.join(folder, 'PACKAGE-MANIFEST.json'), 'utf8'));
for (const file of manifest.files) {
  assert.equal(statSync(path.join(folder, file.path)).size, file.bytes, file.path);
  assert.equal(sha256(path.join(folder, file.path)), file.sha256, file.path);
}
const names = listFiles(folder);
assert(!names.some(file => /steam_api(?:64)?\.dll$/i.test(file)), 'Valve runtime must be excluded');
assert(!names.some(file => /boiler-writter\.exe$/i.test(file)), 'Uncleared Steamworks-linked helper executable must be excluded');
assert(!names.some(file => /(?:^|\/)steamworks(?:_sdk)?(?:\/|$)|(?:^|\/)(?:steam_api|isteamgamecoordinator)\.h$/i.test(file)), 'Proprietary Valve SDK inputs must be excluded');
assert(!names.some(file => forbiddenRuntimePath(file)), 'Private config/cache/pyc/pth excluded');
assert(!names.some(file => /^(?:data|state|cache|models|logs|research|screenshots)\//i.test(file)), 'No user/runtime content');
assert(!names.some(file => /pyvenv\.cfg|direct_url\.json/i.test(file)), 'No machine-specific venv metadata');
for (const file of ['learning.py', 'community.py', 'ai.py', 'evidence.py', 'steam.py', 'storage.py']) assert(existsSync(path.join(folder, 'app/automation', file)), 'Production companion source included');
for (const file of ['trainer.py', 'sandbox.py', 'validator_worker.py', 'Containerfile.validator', 'requirements.txt']) assert(existsSync(path.join(folder, 'app/training', file)), 'Gated operator source included');
for (const module of ['av', 'ctranslate2', 'faster_whisper', 'onnxruntime', 'numpy', 'pandas', 'demoparser2']) assert(!existsSync(path.join(folder, 'tools/python/Lib/site-packages', module)), `Optional ${module} excluded`);
for (const denied of ['C:/Users/Public/ClipForge', 'J:/CS2ClipStudio/artifacts', 'J:/CS2ClipStudio/artifacts-not-owned/ClipForge']) assert.throws(() => assertOwnedOutput(denied));
const repo = path.resolve(import.meta.dirname, '..');
for (const file of manifest.files.filter(item => /^app\/(?:automation|training)\//.test(item.path))) {
  assert.equal(file.sha256, sha256(path.join(repo, file.path.slice(4))), 'Production companion source must match the final build input');
}
const rejectedBuild = path.resolve('J:/CS2ClipStudio/artifacts/public-policy-rejection-fixture');
assert.throws(() => buildPublic({ repo, exe: path.join(folder, 'ClipForge.exe'), output: rejectedBuild, expectedExeSha: '0'.repeat(64) }), /accepted build hash/);
assert(!existsSync(rejectedBuild), 'Hash rejection must precede staging writes');
const oldExecutable = 'J:/CS2ClipStudio/artifacts/ClipForge-0.2.1/ClipForge-0.2.1.exe';
if (existsSync(oldExecutable)) {
  assert.throws(() => buildPublic({ repo, exe: oldExecutable, output: rejectedBuild }), /version does not match/);
  assert(!existsSync(rejectedBuild), 'Legacy executable must not become a mislabeled public package');
}
for (const rejected of ['Lib/site-packages/profile.pth', 'Lib/sitecustomize.py', 'Lib/x/__pycache__/cache.pyc', '.git/config']) assert(forbiddenRuntimePath(rejected));
assert(!forbiddenRuntimePath('Lib/json/__init__.py'));
const helper = JSON.parse(readFileSync(path.join(folder, 'tools/boiler/PROVENANCE.json'), 'utf8'));
assert.equal(helper.valveRuntimeBundled, false);
assert.equal(helper.executableBundled, false);
assert.equal(helper.linkingExceptionFound, false);
assert.equal(sha256(path.join(folder, 'sources/protobuf-cpp-3.20.3.zip')), '2c65db747b0fec51e84480b7092aaff88b072fb27f9187073a219e09bb748bdf');
for (const file of ['CSGOClient.cpp', 'CSGOClient.h', 'GCMsgHandler.h']) assert(readFileSync(path.join(folder, 'sources/boiler-writter-1.7.0/boiler-writter', file), 'utf8').includes('Copyright (C) 2015  Ansas Bogdan'), 'Inherited GPL copyright must remain');
for (const file of ['boiler-GPL-3.0.txt', 'protobuf-3.20.3-BSD-3-Clause.txt', 'mpdecimal-2.5.1-BSD-2-Clause.txt', 'Expat-2.8.3-MIT.txt', 'zlib-1.3.2.txt']) assert(existsSync(path.join(folder, 'licenses/packaged-dependencies', file)), 'Native/helper dependency license retained');
assert(!JSON.stringify(helper).includes('C:\\Users\\'));
assert(!JSON.stringify(helper).includes('J:\\CS2ClipStudio'));
for (const notice of ['START-HERE.txt', 'UPSTREAM.md', 'THIRD-PARTY-NOTICES.md', 'PUBLICATION-CHECKLIST.txt']) {
  const text = readFileSync(path.join(folder, notice), 'utf8');
  const localAccount = os.userInfo().username;
  if (localAccount.length >= 4) assert(!text.toLowerCase().includes(localAccount.toLowerCase()), 'No private local account in public notice');
  assert(!text.includes('J:\\CS2ClipStudio'), 'Public notices must describe relocatable paths');
  assert(!/CSGO(?:-[A-Za-z0-9]{5}){5}|\bgh[pousr]_[A-Za-z0-9_]{20,}|\bsk-proj-[A-Za-z0-9_-]{20,}/.test(text), 'No credentials');
}
const testRoot = path.resolve('J:/CS2ClipStudio/temp/public-package-tests', new Date().toISOString().replaceAll(':', '-'));
mkdirSync(testRoot, { recursive: true });
const wheelFixture = path.join(testRoot, 'wheel-input');
mkdirSync(path.join(wheelFixture, 'fixture'), { recursive: true });
mkdirSync(path.join(wheelFixture, 'fixture.dist-info'));
const verifiedFile = Buffer.from('verified synthetic module\n');
writeFileSync(path.join(wheelFixture, 'fixture/__init__.py'), verifiedFile);
writeFileSync(path.join(wheelFixture, 'fixture/private-settings.json'), 'fixture private data');
writeFileSync(path.join(wheelFixture, 'fixture.dist-info/RECORD'), `fixture/__init__.py,sha256=${createHash('sha256').update(verifiedFile).digest('base64url')},${verifiedFile.length}\n`);
const records = wheelRecords(wheelFixture, 'fixture.dist-info');
const curated = path.join(testRoot, 'curated-wheel');
copyTree(path.join(wheelFixture, 'fixture'), curated, wheelAllow(wheelFixture, 'fixture', records));
assert(existsSync(path.join(curated, '__init__.py')));
assert(!existsSync(path.join(curated, 'private-settings.json')), 'Unregistered local dependency files must not ship');
writeFileSync(path.join(wheelFixture, 'fixture/__init__.py'), 'changed synthetic module');
assert.throws(() => copyTree(path.join(wheelFixture, 'fixture'), path.join(testRoot, 'changed-wheel'), wheelAllow(wheelFixture, 'fixture', records)), /changed from its wheel/);
const moved = path.join(testRoot, 'Moved studio with spaces');
cpSync(folder, moved, { recursive: true });
const env = { ...process.env, CLIPFORGE_WORKSPACE: moved, PYTHONPATH: '', PYTHONHOME: '', PYTHONNOUSERSITE: '1', PYTHONDONTWRITEBYTECODE: '1', TEMP: path.join(testRoot, 'temp'), TMP: path.join(testRoot, 'temp'), APPDATA: path.join(testRoot, 'appdata'), LOCALAPPDATA: path.join(testRoot, 'localappdata'), HF_HOME: path.join(testRoot, 'hf'), XDG_CACHE_HOME: path.join(testRoot, 'cache') };
for (const directory of [env.TEMP, env.APPDATA, env.LOCALAPPDATA, env.HF_HOME, env.XDG_CACHE_HOME]) mkdirSync(directory, { recursive: true });
const python = path.join(moved, 'tools/python/python.exe');
const probe = 'import sys,pathlib,json,ssl,sqlite3,bz2,ctypes,httpx,httpcore,h11,anyio,idna,certifi,psutil,pip,typing_extensions\nroot=pathlib.Path(sys.executable).resolve().parent\nassert pathlib.Path(sys.prefix).resolve()==root\nfor mod in (httpx,httpcore,h11,anyio,idna,certifi,psutil,pip,typing_extensions):\n assert pathlib.Path(mod.__file__).resolve().is_relative_to(root),mod.__name__\nassert not any("python-env" in p or "codex-runtimes" in p or "AppData" in p for p in sys.path)\nprint(json.dumps({"prefixOwned":True,"imports":9,"ssl":ssl.OPENSSL_VERSION,"sqlite":sqlite3.sqlite_version}))';
const imports = spawnSync(python, ['-I', '-B', '-c', probe], { encoding: 'utf8', cwd: moved, env, timeout: 30000, windowsHide: true });
assert.equal(imports.status, 0, imports.stderr);
const cli = path.join(moved, 'app/automation/cli.py');
function request(value) {
  const result = spawnSync(python, ['-s', '-B', '-u', cli], { cwd: path.dirname(cli), env, input: JSON.stringify(value), encoding: 'utf8', timeout: 30000, windowsHide: true });
  assert.equal(result.status, 0, result.stderr);
  const response = JSON.parse(result.stdout);
  assert.equal(response.ok, true, response.error);
  return response.result;
}
const status = request({ command: 'status' });
assert.equal(status.settings.enabled, false);
assert.equal(status.capabilities.matchHistory, false, 'Source-only public package must disable match-history download');
assert.equal(status.matches.length, 0);
assert.equal(status.models.runtimeInstalled, false);
assert(status.models.models.every(model => !model.installed));
const community = request({ command: 'community', action: 'status', values: {} });
assert.equal(community.contributions.length, 0);
const helperProbe = 'import steam,pathlib,ai\nassert ai.PIPELINE.startswith("clipforge-local-vision-v8-"),"Final validated vision pipeline must be included"\nassert not steam.BOILER.is_file()\ntry:\n steam.prepare_installed_runtime(None)\nexcept steam.SteamError as error:\n assert "Import demo" in str(error),str(error)\nelse:\n raise AssertionError("Excluded helper must provide explicit public import guidance")\nassert not (steam.BOILER.parent/"steam_api64.dll").exists()\nprint("missing helper guidance without Valve staging passed")';
const missingHelper = spawnSync(python, ['-s', '-B', '-c', helperProbe], { cwd: path.dirname(cli), env, encoding: 'utf8', timeout: 30000, windowsHide: true });
assert.equal(missingHelper.status, 0, missingHelper.stderr);
assert(existsSync(path.join(moved, 'state/automation/automation.db')), 'State created only inside relocated workspace');
assert(!existsSync(path.join(folder, 'state')), 'Shipped staging remains pristine');
const report = { executableSha256: sha256(path.join(folder, 'ClipForge.exe')), packageFiles: manifest.files.length, packageBytes: manifest.files.reduce((n, item) => n + item.bytes, 0), privacyAndAllowlist: 'passed', runtimeImports: JSON.parse(imports.stdout), movedCliStatus: 'passed', movedCommunityStatus: 'passed', publicMatchHistoryDisabled: 'passed', validatedVisionPipelineV8Included: 'passed', missingHelperImportGuidanceWithoutValveStaging: 'passed', nativeGuiCoverage: 'Not covered by this headless test; exact executable requires a separate actual fresh-workspace GUI smoke without a development server', fixtureRoot: testRoot };
writeFileSync(path.join(path.dirname(folder), 'PORTABLE-VERIFICATION.json'), JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify(report));
