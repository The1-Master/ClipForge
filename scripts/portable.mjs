// Preserve the private workspace package; --public creates a separate relocatable distribution.
import { copyFileSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { assertOwnedOutput, buildPublic, listFiles, sha256 } from '../packaging/public/build.mjs';

const repo = path.resolve(import.meta.dirname, '..');
const { version } = JSON.parse(readFileSync(path.join(repo, 'package.json'), 'utf8'));
const args = process.argv.slice(2);
const noticesOnly = args.includes('--notices-only');
const publicPackage = args.includes('--public');
function option(name) { const index = args.indexOf(name); return index < 0 ? undefined : args[index + 1]; }
const positional = args.find((value, index) => !value.startsWith('--') && !['--output', '--base-python', '--package-source', '--expected-exe-sha'].includes(args[index - 1]));
const exe = path.resolve(repo, positional ?? 'target/release/demodesk.exe');
if (publicPackage) {
  if (noticesOnly) throw new Error('Public packages require a complete executable and clean staging folder');
  const output = assertOwnedOutput(option('--output') ?? path.join('J:/CS2ClipStudio/artifacts', `ClipForge-${version}-public`, 'ClipForge'));
  if (args.includes('--replace-staging') && existsSync(output)) {
    const manifest = JSON.parse(readFileSync(path.join(output, 'PACKAGE-MANIFEST.json'), 'utf8'));
    if (manifest.product !== 'ClipForge') throw new Error('Cannot replace an unknown artifact');
    const declared = new Set([...manifest.files.map(file => file.path), 'PACKAGE-MANIFEST.json']);
    if (listFiles(output).some(file => !declared.has(file))) throw new Error('Staging contains unregistered user/runtime files; preserve it separately');
    for (const file of manifest.files) if (sha256(path.join(output, file.path)) !== file.sha256) throw new Error('Staging files changed; preserve the folder separately');
    const preserved = path.join(path.dirname(output), `previous-staging-${Date.now()}`);
    assertOwnedOutput(preserved);
    renameSync(output, preserved);
  }
  const result = buildPublic({ repo, exe, output, expectedExeSha: option('--expected-exe-sha'), basePython: option('--base-python'), packageSource: option('--package-source') });
  console.log(JSON.stringify(result));
} else {
  const output = path.join('J:/CS2ClipStudio/artifacts', `ClipForge-${version}`);
  mkdirSync(output, { recursive: true });
  const destination = path.join(output, `ClipForge-${version}.exe`);
  if (!noticesOnly) copyFileSync(exe, destination);
  for (const file of ['LICENSE', 'UPSTREAM.md', 'IMPLEMENTATION.md', 'THIRD-PARTY-NOTICES.md']) copyFileSync(path.join(repo, file), path.join(output, file === 'LICENSE' ? 'LICENSE.txt' : file));
  const licenses = path.join(output, 'licenses');
  mkdirSync(licenses, { recursive: true });
  for (const [file, name] of [
    ['vendor/demoparser/LICENSE', 'demoparser-MIT.txt'], ['public/fonts/Inter-LICENSE.txt', 'Inter-4.1-OFL.txt'],
    ['assets/branding/Bootstrap-Icons-LICENSE.txt', 'Bootstrap-Icons-MIT.txt'],
    ['crates/demodesk-core/window-hook/Detours.LICENSE.md', 'Detours-MIT.txt'],
    ['crates/demodesk-core/src/render/CSDM.LICENSE.txt', 'CS-Demo-Manager-MIT.txt'],
    ['crates/demodesk-core/src/smoke/LICENSE', 'smoke-GPL-3.0.txt'],
    ['public/weapons/LICENSE', 'Counter-Strike-assets-and-tooling.txt'],
  ]) copyFileSync(path.join(repo, file), path.join(licenses, name));
  writeFileSync(path.join(output, 'START-HERE.txt'), [
    `ClipForge ${version} — private development workspace package`, '',
    'Run this executable with the existing complete development workspace intact.',
    'This private artifact excludes helpers/models/data; it is not the public relocatable package.',
    'The separate -public artifact contains the core Python runtime and match helper.',
    'Automatic processing remains OFF. Keep existing user data, credentials and models private.',
    'Read LICENSE.txt, UPSTREAM.md, THIRD-PARTY-NOTICES.md and IMPLEMENTATION.md.', '',
  ].join('\r\n'));
  console.log(noticesOnly ? `Private notices refreshed: ${output}` : `Private executable packaged: ${destination}`);
}
