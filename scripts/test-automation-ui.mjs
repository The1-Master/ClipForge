import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {mkdir} from 'node:fs/promises';
import {createServer} from 'vite';
const require=createRequire(import.meta.url);
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const server=await createServer({cacheDir:`out/automation-ui-cache-${process.pid}`,server:{host:'127.0.0.1',port:0,watch:{ignored:['**/*']},hmr:false}});
await server.listen();
const browser=await chromium.launch({channel:'chrome',headless:true});
try {
  const page=await browser.newPage({viewport:{width:1440,height:1080}});
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  await page.addInitScript(()=>{
    const callbacks=new Map();let id=0;
    window.automation={settings:{steamId:'7656119'+'8000000001',lastShareCode:'',enabled:false,
      autoExport:true,saveDeaths:true,minKills:2,aiEnabled:true,aiThreshold:70,model:'qwen3-vl:8b-instruct',
      apiKeySet:false,authCodeSet:false},matches:[],models:{runtimeInstalled:false,models:[]},
      gameOpen:true,gamePresence:'external',queuePaused:false,activity:{busy:false}};
    window.calls=[];
    window.__TAURI_EVENT_PLUGIN_INTERNALS__={unregisterListener:()=>{}};
    window.__TAURI_INTERNALS__={metadata:{currentWindow:{label:'main'},currentWebview:{label:'main'}},
      transformCallback(callback){const key=++id;callbacks.set(key,callback);return key;},
      unregisterCallback(key){callbacks.delete(key);},convertFileSrc:path=>path,
      async invoke(cmd,args={}){
        window.calls.push({cmd,args});
        if(cmd==='plugin:event|listen')return ++id;
        if(cmd==='plugin:event|unlisten')return;
        if(cmd==='get_startup_error')return null;
        if(cmd==='preferences_need_reset')return false;
        if(['list_demos','analysis_jobs'].includes(cmd))return [];
        if(cmd==='list_jobs')return [{id:'queued-personal-test',demoId:'missing-test-match',highlightIds:['moment'],status:'queued',options:{width:3840,height:2160,fps:60},outputs:[],log:[],createdAt:'2026-10-12T10:00:00Z'}];
        if(cmd==='get_status')return {ok:true,missingRenderTools:[],problems:[],dataDir:'J:/CS2ClipStudio/data/clipforge',version:'0.1.0'};
        if(cmd==='get_settings')return {settings:{language:'en',replayFolders:[],scanGameReplays:false,
          steamDir:null,cs2Dir:null,hlaeExe:null,ffmpegExe:null,vrfExe:null},setup:{},toolChecks:{},
          doctor:{ok:true,problems:[],paths:{}},detected:{},dataDir:'J:/CS2ClipStudio/data/clipforge',restartRequired:false};
        if(cmd==='get_storage_bytes')return {parsedBytes:0,anomalyBytes:0,clipsBytes:0,radarBytes:0};
        if(cmd==='automation_status')return JSON.parse(JSON.stringify(window.automation));
        if(cmd==='automation_save'){
          const values={...args.values};
          if(values.apiKey){window.automation.settings.apiKeySet=true;delete values.apiKey;}
          if(values.authCode){window.automation.settings.authCodeSet=true;delete values.authCode;}
          Object.assign(window.automation.settings,values);return JSON.parse(JSON.stringify(window.automation));
        }
        if(cmd==='pause_queue'){window.automation.queuePaused=args.paused;return;}
        if(cmd==='automation_action'){window.automation.activity={busy:true,progress:'Waiting safely for CS2 to close'};return;}
        if(cmd==='automation_cancel'){window.automation.activity={busy:true,cancelling:true,progress:'Cancelling…'};return;}
        throw new Error('Unexpected test action: '+cmd);
      }};
  });
  await page.goto(server.resolvedUrls.local[0]);
  const navigation=page.getByRole('navigation',{name:'Main navigation'});
  await navigation.getByRole('button',{name:'Settings',exact:true}).click();
  await page.getByLabel('Steam ID64',{exact:true}).waitFor();
  await page.getByRole('navigation',{name:'Settings sections'}).getByRole('button',{name:/^Automation/}).click();
  assert.equal(await page.getByRole('checkbox',{name:/Automatically process new matches/}).isChecked(),false,'Automatic processing starts disabled');
  await page.getByRole('navigation',{name:'Settings sections'}).getByRole('button',{name:/^Connection/}).click();
  await page.getByLabel('Steam Web API key',{exact:true}).fill('synthetic-api-key');
  await page.getByLabel('Match-history authentication code',{exact:true}).fill('synthetic-auth-code');
  await page.getByRole('button',{name:'Save changes',exact:true}).click();
  await page.getByRole('button',{name:'Save changes',exact:true,disabled:true}).waitFor();
  assert.equal(await page.getByLabel(/Steam Web API key/).inputValue(),'','Secret entry is cleared after saving');
  assert.equal(await page.getByLabel(/Match-history authentication code/).getAttribute('type'),'password');
  assert.equal(await page.evaluate(()=>window.calls.filter(c=>c.cmd==='automation_save').at(-1).args.values.apiKeySet),undefined);
  await navigation.getByRole('button',{name:/^Activity/}).click();
  await page.getByRole('button',{name:'Pause next exports',exact:true}).click();
  await page.getByRole('button',{name:'Resume next exports',exact:true}).waitFor();
  await page.getByRole('button',{name:'Resume next exports',exact:true}).click();
  await navigation.getByRole('button',{name:'Home',exact:true}).click();
  await page.getByRole('status').filter({hasText:'Waiting for CS2 to close'}).first().waitFor();
  await page.getByRole('button',{name:'Download matches',exact:true}).click();
  await navigation.getByRole('button',{name:/^Activity/}).click();
  await page.getByRole('button',{name:'Cancel task',exact:true}).click();
  await page.getByRole('button',{name:'Cancelling…',exact:true,disabled:true}).waitFor();
  await page.evaluate(()=>{window.automation.activity={busy:false,progress:'Cancelled'};});
  await page.getByRole('button',{name:'Cancelling…',exact:true}).waitFor({state:'detached'});
  const screenshot=process.env.UI_AUDIT_OUTPUT_DIR?process.env.UI_AUDIT_OUTPUT_DIR+'/automation-ui.png':'J:/CS2ClipStudio/research/benchmarks/desktop/automation-ui-0.2.0.png';
  await mkdir(process.env.UI_AUDIT_OUTPUT_DIR||'J:/CS2ClipStudio/research/benchmarks/desktop',{recursive:true});
  await page.screenshot({path:screenshot,fullPage:true});
  assert.deepEqual(errors,[]);
  console.log('Automation UI passed: defaults, masked credential save, queue pause/resume, waiting and cancellation.');
} finally {await browser.close();await server.close();}
