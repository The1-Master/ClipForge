// Verify a downloaded release against an independently trusted signing key.
import {createHash, verify} from 'node:crypto';
import {createReadStream} from 'node:fs';
import {lstat, readFile, realpath} from 'node:fs/promises';
import path from 'node:path';

export async function verifyRelease(folder, trustedKeyPath) {
  const root = await realpath(folder);
  const manifestPath = path.join(root, 'release-manifest.json');
  if ((await lstat(manifestPath)).size > 64 * 1024) throw Error('Manifest exceeds its limit');
  const manifest = await readFile(manifestPath);
  const key = await readFile(trustedKeyPath);
  const signature = await readFile(path.join(root, 'release-manifest.sig'), 'utf8');
  if (!/^[A-Za-z0-9+/]{86}==\s*$/.test(signature)) throw Error('Invalid Ed25519 signature');
  if (!verify(null, manifest, key, Buffer.from(signature.trim(), 'base64'))) throw Error('Manifest signature failed');
  const document = JSON.parse(manifest);
  if (document.schemaVersion !== 1 || document.algorithm !== 'Ed25519'
      || document.repository !== 'The1-Master/ClipForge' || !Array.isArray(document.artifacts)
      || document.artifacts.length < 1 || document.artifacts.length > 16) throw Error('Unsupported manifest');
  const seen = new Set();
  for (const artifact of document.artifacts) {
    if (!/^ClipForge-[A-Za-z0-9_.-]+\.(?:exe|zip)$/.test(artifact.name)
        || seen.has(artifact.name) || !/^[a-f0-9]{64}$/.test(artifact.sha256)
        || !Number.isSafeInteger(artifact.bytes) || artifact.bytes < 1) throw Error('Invalid artifact record');
    seen.add(artifact.name);
    const file = path.join(root, artifact.name);
    const metadata = await lstat(file);
    if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size !== artifact.bytes) throw Error(`File size/type mismatch: ${artifact.name}`);
    const digest = createHash('sha256');
    for await (const bytes of createReadStream(file)) digest.update(bytes);
    if (digest.digest('hex') !== artifact.sha256) throw Error(`Checksum failed: ${artifact.name}`);
  }
  return document.artifacts.length;
}

if (process.argv[1] && path.resolve(process.argv[1]) === path.resolve(import.meta.filename)) {
  const [folder, trustedKeyPath] = process.argv.slice(2);
  if (!folder || !trustedKeyPath) throw Error('Usage: node verify-release.mjs <release folder> <independently trusted public key>');
  console.log(`Verified ${await verifyRelease(folder, trustedKeyPath)} signed release artifacts.`);
}
