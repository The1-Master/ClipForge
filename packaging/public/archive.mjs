import { existsSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { assertOwnedOutput, listFiles, sha256 } from './build.mjs';

const folder = assertOwnedOutput(process.argv[2]);
const parent = path.dirname(folder);
const marker = JSON.parse(readFileSync(path.join(folder, 'clipforge-portable.json'), 'utf8'));
const zip = path.join(parent, `ClipForge-${marker.version}-Windows-x64.zip`);
const setup = path.join(parent, `ClipForge-${marker.version}-Setup.exe`);
const python = path.join(folder, 'tools/python/python.exe');
const source = `import pathlib,sys,zipfile\nroot=pathlib.Path(sys.argv[1]); out=pathlib.Path(sys.argv[2])\nwith zipfile.ZipFile(out,'w',compression=zipfile.ZIP_DEFLATED,compresslevel=6) as z:\n for file in sorted(root.rglob('*')):\n  if file.is_symlink(): raise RuntimeError('Package link rejected')\n  if file.is_file(): z.write(file,pathlib.Path(root.name)/file.relative_to(root))\n`;
const result = spawnSync(python, ['-I', '-B', '-c', source, folder, zip], { encoding: 'utf8', timeout: 180000, env: { ...process.env, PYTHONPATH: '', PYTHONHOME: '', PYTHONNOUSERSITE: '1', TEMP: 'J:/CS2ClipStudio/temp', TMP: 'J:/CS2ClipStudio/temp' } });
if (result.status !== 0) throw new Error('ZIP creation failed: ' + result.stderr);
const deletion = path.join(parent, 'uninstall-files.nsh');
const checks = path.join(parent, 'check-destinations.nsh');
const quote = text => text.replaceAll('$', '$$').replaceAll('"', '$\\"');
const packageFiles = listFiles(folder);
writeFileSync(deletion, packageFiles.filter(file => file !== 'clipforge-portable.json').map(file => `Delete "$INSTDIR\\${quote(file.replaceAll('/', '\\'))}"`).join('\r\n') + '\r\n');
const destinations = new Set(packageFiles);
for (const file of packageFiles) {
  let directory = path.posix.dirname(file);
  while (directory !== '.') { destinations.add(directory); directory = path.posix.dirname(directory); }
}
writeFileSync(checks, [...destinations].sort().map(file => `!insertmacro CheckNoLink "${quote(file.replaceAll('/', '\\'))}"`).join('\r\n') + '\r\n');
const nsis = 'J:/CS2ClipStudio/tools/nsis/nsis-3.12/Bin/makensis.exe';
if (!existsSync(nsis)) throw new Error('Verified NSIS compiler is unavailable');
const installer = spawnSync(nsis, ['/V2', `/DPACKAGE_ROOT=${folder}`, `/DSETUP_OUT=${setup}`, `/DVERSION=${marker.version}`, `/DDELETE_MANIFEST=${deletion}`, `/DCHECK_MANIFEST=${checks}`, `/DNATIVE_ICON=${path.resolve(import.meta.dirname, '../../src-tauri/icons/icon.ico')}`, path.join(import.meta.dirname, 'installer.nsi')], { encoding: 'utf8', timeout: 300000, windowsHide: true, env: { ...process.env, TEMP: 'J:/CS2ClipStudio/temp', TMP: 'J:/CS2ClipStudio/temp' } });
writeFileSync(path.join(parent, 'installer-build.log'), installer.stdout + installer.stderr);
if (installer.status !== 0) throw new Error('Installer build failed; inspect the private artifact build log');
const files = [zip, setup, path.join(folder, 'ClipForge.exe'), path.join(folder, 'PACKAGE-MANIFEST.json')].map(file => ({ file: path.basename(file), bytes: statSync(file).size, sha256: sha256(file) }));
writeFileSync(path.join(parent, 'SHA256SUMS.json'), JSON.stringify({ version: marker.version, files }, null, 2) + '\n');
writeFileSync(path.join(parent, 'SHA256SUMS.txt'), files.map(file => `${file.sha256}  ${file.file}`).join('\r\n') + '\r\n');
console.log(JSON.stringify({ zip, setup, files }));
