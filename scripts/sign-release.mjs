// Sign reviewed public artifacts. The private key remains DPAPI-protected locally.
import {createHash,createPrivateKey,generateKeyPairSync,sign,verify} from 'node:crypto';
import {existsSync,mkdirSync,readFileSync,readdirSync,statSync,writeFileSync} from 'node:fs';
import path from 'node:path';
import {spawnSync} from 'node:child_process';
const workspace='J:/CS2ClipStudio';
const folder=path.resolve(process.argv[2]??'');
if(!folder.startsWith(path.resolve(workspace) + path.sep)||!folder.includes('artifacts'))throw new Error('Choose a reviewed workspace artifact folder');
const keyPath=path.join(workspace,'state/release/signing-key.dpapi');
mkdirSync(path.dirname(keyPath),{recursive:true});
function protect(value,decrypt=false){
  const code=`import sys;sys.path.insert(0,'automation');import storage;print(storage.protect(sys.stdin.read(),${decrypt?'True':'False'}))`;
  const result=spawnSync(path.join(workspace,'tools/python-env/Scripts/python.exe'),['-c',code],{cwd:path.resolve(workspace,'app'),input:value,encoding:'utf8',windowsHide:true,env:{...process.env,PYTHONNOUSERSITE:'1',PYTHONPYCACHEPREFIX:path.join(workspace,'cache/pycache')}});
  if(result.status!==0)throw new Error('Cannot protect release signing credentials');
  return result.stdout.trim();
}
let key;
if(existsSync(keyPath))key=createPrivateKey({key:Buffer.from(protect(readFileSync(keyPath,'utf8'),true),'base64'),format:'der',type:'pkcs8'});
else{
  key=generateKeyPairSync('ed25519').privateKey;
  writeFileSync(keyPath,protect(key.export({format:'der',type:'pkcs8'}).toString('base64')),{flag:'wx'});
}
const {createPublicKey}=await import('node:crypto');
const publicKey=createPublicKey(key).export({format:'pem',type:'spki'});
const files=readdirSync(folder).filter(name=>/^ClipForge-.*\.(?:exe|zip)$/.test(name)).sort().map(name=>{
  const file=path.join(folder,name);const bytes=readFileSync(file);
  return {name,bytes:statSync(file).size,sha256:createHash('sha256').update(bytes).digest('hex')};
});
if(!files.length)throw new Error('No reviewed release artifacts found');
const manifest=Buffer.from(JSON.stringify({schemaVersion:1,version:'0.3.0-beta.1',repository:'The1-Master/ClipForge',algorithm:'Ed25519',artifacts:files},null,2)+'\n');
const signature=sign(null,manifest,key);
if(!verify(null,manifest,publicKey,signature))throw new Error('Release signature self-check failed');
writeFileSync(path.join(folder,'release-manifest.json'),manifest);
writeFileSync(path.join(folder,'release-manifest.sig'),signature.toString('base64')+'\n');
writeFileSync(path.join(folder,'release-public-key.pem'),publicKey);
writeFileSync(path.join(folder,'SHA256SUMS'),files.map(file=>`${file.sha256}  ${file.name}`).join('\n')+'\n');
console.log(`Signed manifest for ${files.length} reviewed artifacts; signing key remains protected locally.`);
