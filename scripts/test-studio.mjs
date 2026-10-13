import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { createServer } from 'vite';
import AxeBuilder from '@axe-core/playwright';
const require=createRequire(import.meta.url);
const {chromium}=require(process.env.PLAYWRIGHT_MODULE||'playwright');
const output=process.env.UI_AUDIT_OUTPUT_DIR||'J:/CS2ClipStudio/research/benchmarks/redesign';
await mkdir(output,{recursive:true});
const server=await createServer({cacheDir:'J:/CS2ClipStudio/cache/studio-ui-vite',server:{host:'127.0.0.1',port:0,watch:{ignored:['**/*']},hmr:false}});
await server.listen();
const browser=await chromium.launch({channel:'chrome',headless:true});
const errors=[];
const checks=[];
try{
  const context=await browser.newContext({viewport:{width:1366,height:768}});
  const page=await context.newPage();
  page.on('pageerror',e=>errors.push(e.message));
  // Reuse the full existing synthetic parser/replay/settings fixture, not user data.
  const original=await readFile(new URL('./test-ui.mjs',import.meta.url),'utf8');
  const begin=original.indexOf('  await page.addInitScript(() => {');
  const end=original.indexOf('  await page.goto(',begin);
  assert.ok(begin>=0&&end>begin,'Shared component fixture is available');
  const AsyncFunction=Object.getPrototypeOf(async function(){}).constructor;
  await new AsyncFunction('page',original.slice(begin,end))(page);
  await page.addInitScript(()=>{
    const base=window.__TAURI_INTERNALS__.invoke;
    const convert=window.__TAURI_INTERNALS__.convertFileSrc;
    window.__TAURI_INTERNALS__.convertFileSrc=path=>path.endsWith('.jpg')?'data:image/svg+xml,'+encodeURIComponent('<svg xmlns="http://www.w3.org/2000/svg" width="640" height="360"><rect width="640" height="360" fill="#1d2a39"/><path d="M0 280 180 130 420 190 640 70V360H0Z" fill="#314d65"/><circle cx="430" cy="110" r="40" fill="#ffb15a"/><text x="32" y="330" fill="#f5f7fc" font-family="sans-serif" font-size="24">Fixture gameplay preview</text></svg>'):convert(path);
    const defaults={fps:60,width:3840,height:2160,codec:'h264_amf',crf:18,container:'mp4',camera:'lock',deathNoticeSeconds:5,hud:true,crosshair:true,radar:true,killFeed:true,chat:true,viewmodel:true,tracers:true,hudScale:.85,trueView:false,xray:false,voice:true,showGame:false,quitWhenDone:true,keepRawFiles:false,merge:false,keyMomentsOnly:true,roundResultLabel:'Round result',maxSizeMb:null,audioKbps:192,extraLaunchOptions:[]};
    window.studioAuto={settings:{steamId:'1',lastShareCode:'',enabled:false,autoExport:false,saveDeaths:true,minKills:2,aiEnabled:false,aiThreshold:80,model:'qwen3-vl:8b-instruct',apiKeySet:false,authCodeSet:true},matches:[{code:'fixture',path:'J:/fixture.dem',status:'downloaded'}],gameOpen:false,gamePresence:'closed',queuePaused:false,activity:{busy:false}};
    window.studioCalls=[];window.pollInFlight=0;window.maxPollInFlight=0;
    window.__TAURI_INTERNALS__.invoke=async(cmd,args={})=>{
      window.studioCalls.push({cmd,args});
      if(cmd==='automation_status'){
        window.pollInFlight++;window.maxPollInFlight=Math.max(window.maxPollInFlight,window.pollInFlight);
        try{if(window.holdAuto)await new Promise(resolve=>window.releaseAuto=resolve);return structuredClone(window.studioAuto);}finally{window.pollInFlight--;}
      }
      if(cmd==='automation_save'){const {apiKey,authCode,...values}=args.values;Object.assign(window.studioAuto.settings,values);if(apiKey)window.studioAuto.settings.apiKeySet=true;if(authCode)window.studioAuto.settings.authCodeSet=true;return structuredClone(window.studioAuto);}
      if(cmd==='automation_action')return;
      if(cmd==='automation_cancel'){window.studioAuto.activity={busy:false,progress:'Cancelled'};if(window.studioAuto.review)window.studioAuto.review.complete=true;return;}
      if(cmd==='pause_queue'){window.studioAuto.queuePaused=args.paused;return;}
      if(cmd==='get_clip_media')return {outputKey:args.outputKey,width:3840,height:2160,fps:60,durationSeconds:8.033333,posterPath:'J:/fixture.jpg'};
      if(cmd==='cancel_clip_media')return;
      if(cmd==='retry_render_job')return;
      if(cmd==='get_settings'||cmd==='check_tools'){const response=await base(cmd,args);response.settings.renderDefaults=window.studioPreset||defaults;return response;}
      if(cmd==='save_settings'){window.studioPreset=args.settings.renderDefaults;return base(cmd,args);}
      if(cmd==='get_demo'){const response=await base(cmd,args);const first=response.parsed?.highlights?.[0];if(first)response.parsed.highlights.push({...first,id:'death-1',score:0,tags:['death'],title:'Player — death · R8'});return response;}
      return base(cmd,args);
    };
  });
  await page.goto(server.resolvedUrls.local[0]);
  const nav=page.getByRole('navigation',{name:'Main navigation'});
  await nav.getByRole('button',{name:'Home',exact:true}).waitFor();
  await page.getByText('Make every moment').waitFor();
  await page.locator('.home-page .clip-card').first().waitFor();
  assert.equal(await page.evaluate(()=>window.testCalls.filter(c=>c.cmd==='get_demo').length),0,'Home never loads full parsed matches');
  assert.ok(await page.evaluate(()=>window.studioCalls.filter(c=>c.cmd==='get_clip_media').length)<20,'Home loads only visible previews');
  await page.evaluate(()=>document.fonts.ready);
  assert.ok(await page.evaluate(()=>document.fonts.check('16px "Inter"')),'Local studio font loads');
  checks.push('home: real navigation, no full-demo preload, bounded poster reads, local font');
  await page.screenshot({path:output+'/home-dark.png'});
  const audit=async(name)=>{
    const result=await new AxeBuilder({page}).withTags(['wcag2a','wcag2aa','wcag21aa','wcag22aa']).analyze();
    await writeFile(output+'/accessibility-'+name+'.json',JSON.stringify(result,null,2));
    assert.deepEqual(result.violations.map(v=>({id:v.id,targets:v.nodes.map(n=>n.target)})),[],name+' accessibility checks');
  };
  await audit('home');
  await nav.getByRole('button',{name:'Clips',exact:true}).click();
  await page.locator('.clip-gallery-scroll').waitFor();
  assert.ok(await page.locator('.clip-gallery-scroll .clip-card').count()<40,'Gallery DOM is bounded');
  assert.equal(await page.locator('.clip-gallery-scroll video').count(),0,'Cards never mount video players');
  await page.getByLabel('Search clips').fill('does-not-exist');
  await page.getByText('No clips match these filters').waitFor();
  await page.getByLabel('Search clips').fill('');
  await page.locator('.clip-gallery-scroll .clip-card').first().click();
  await page.locator('.clip-detail-stage video').waitFor();
  assert.equal(await page.locator('video').count(),1,'Only the selected clip mounts a player');
  await page.getByRole('button',{name:'All clips',exact:true}).click();
  checks.push('gallery: virtualized, lazy posters, filtering, one player and return');
  await nav.getByRole('button',{name:'Matches',exact:true}).click();
  await page.locator('.demo-item').first().click();
  await page.locator('.demo-tabs').waitFor();
  assert.equal(await page.getByRole('tab',{name:/Moments/}).getAttribute('aria-selected'),'true');
  await page.getByRole('radio',{name:/Deaths/}).click();
  await page.getByRole('cell',{name:'death',exact:true}).first().waitFor();
  checks.push('matches: Moments default, personal selection, death outside score threshold');
  await page.screenshot({path:output+'/matches-dark.png'});
  await nav.getByRole('button',{name:'Settings',exact:true}).click();
  await page.getByLabel('Steam ID64',{exact:true}).fill('12345678901234567');
  await page.getByRole('navigation',{name:'Settings sections'}).getByRole('button',{name:/AI /}).click();
  await page.getByRole('navigation',{name:'Settings sections'}).getByRole('button',{name:/Connection/}).click();
  assert.equal(await page.getByLabel('Steam ID64',{exact:true}).inputValue(),'12345678901234567','Draft survives settings categories');
  await nav.getByRole('button',{name:'Home',exact:true}).click();
  await page.getByRole('alertdialog').getByText('Keep your settings changes?').waitFor();
  await page.getByRole('button',{name:'Stay here'}).click();
  assert.equal(await page.getByLabel('Steam ID64',{exact:true}).inputValue(),'12345678901234567');
  await nav.getByRole('button',{name:'Home',exact:true}).click();
  await page.getByRole('button',{name:'Discard and leave'}).click();
  await nav.getByRole('button',{name:'Settings',exact:true}).click();
  assert.equal(await page.getByLabel('Steam ID64',{exact:true}).inputValue(),'1');
  checks.push('settings: category draft persistence and Stay/Discard navigation guard');
  const sections=page.getByRole('navigation',{name:'Settings sections'});
  for(const name of ['Connection','Automation','AI','Recording','Appearance','Tools','Storage','About']){
    await sections.getByRole('button',{name:new RegExp('^'+name+'(?: |$)')}).click();
    await audit('settings-'+name.toLowerCase());
  }
  await sections.getByRole('button',{name:/^Tools/}).click();
  const settingsScroll=page.locator('.settings-shell');
  await settingsScroll.evaluate(element=>element.scrollTop=0);
  const scrollBounds=await settingsScroll.boundingBox();
  assert.ok(scrollBounds);
  await page.mouse.move(scrollBounds.x+scrollBounds.width*.8,scrollBounds.y+Math.min(scrollBounds.height,300)*.5);
  await page.mouse.wheel(0,1200);
  await page.waitForFunction(()=>document.querySelector('.settings-shell').scrollTop>0);
  assert.equal(await page.locator('.studio-main').evaluate(element=>element.scrollTop),0,'Wheel scrolls Settings rather than the clipped workspace');
  checks.push('settings: ordinary wheel scrolling reaches lower Tools controls');
  await sections.getByRole('button',{name:/^Appearance/}).click();
  await page.getByRole('button',{name:'Light studio',exact:true}).click();
  await nav.getByRole('button',{name:'Home',exact:true}).click();
  await audit('home-light');
  await page.screenshot({path:output+'/home-light.png'});
  for(const size of [{width:960,height:640},{width:1920,height:1080}]){
    await page.setViewportSize(size);
    assert.equal(await nav.getByRole('button',{name:'Matches',exact:true}).isVisible(),true,'Navigation remains labeled');
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Shell has no horizontal clipping');
    await page.screenshot({path:output+'/home-'+size.width+'.png'});
  }
  await page.setViewportSize({width:640,height:640});
  await page.getByRole('button',{name:'Menu',exact:true}).click();
  await nav.getByRole('button',{name:'Settings',exact:true}).click();
  await sections.getByRole('button',{name:/^Appearance/}).click();
  await page.getByRole('button',{name:'Large',exact:true}).click();
  await page.emulateMedia({reducedMotion:'reduce'});
  await page.screenshot({path:output+'/large-text-reduced-motion.png'});
  assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Large text and narrow layout fit');
  checks.push('responsive: 960,1366,1920, narrow drawer, large text, light/dark, reduced motion');
  await page.setViewportSize({width:1366,height:768});
  await nav.getByRole('button',{name:'Activity',exact:true}).click();
  await page.getByText('Your processing queue').waitFor();
  await page.screenshot({path:output+'/activity.png'});
  await audit('activity');
  await page.evaluate(()=>{window.studioAuto.review={demoId:'demo-0',jobId:'job-1',playerId:'1',status:'Preview queued',complete:false,moments:[]};window.studioAuto.activity={busy:false};});
  await page.getByRole('button',{name:'Cancel task',exact:true}).waitFor();
  await page.getByRole('button',{name:'Cancel task',exact:true}).click();
  assert.ok(await page.evaluate(()=>window.studioCalls.some(c=>c.cmd==='automation_cancel')),'Review preview cancellation is exposed');
  await page.evaluate(()=>{window.holdAuto=true;});
  await page.waitForFunction(()=>typeof window.releaseAuto==='function');
  await page.waitForTimeout(3200);
  assert.equal(await page.evaluate(()=>window.maxPollInFlight),1,'Slow polls never overlap');
  await page.evaluate(()=>{window.holdAuto=false;window.releaseAuto();});
  checks.push('activity: persists across routes, review-preview cancel, single-flight slow poll');
  await nav.getByRole('button',{name:'Home',exact:true}).click();
  await page.evaluate(async()=>{
    const job=(await window.__TAURI_INTERNALS__.invoke('list_jobs'))[0];
    const failed={...job,status:'error',stage:'encoding',error:'Fixture encoder stopped'};
    window.emitTestEvent({type:'job-changed',job:failed});
    window.emitTestEvent({type:'job-changed',job:failed});
  });
  const failedNotice=page.locator('.app-toast[role="alert"]').filter({hasText:'An export needs attention'});
  await failedNotice.waitFor();
  assert.equal(await failedNotice.count(),1,'A terminal export failure is visible once on Home');
  await nav.getByRole('button',{name:'Activity',exact:true}).click();
  assert.equal(await failedNotice.count(),1,'Failure notice survives navigation until dismissed');
  await failedNotice.getByRole('button',{name:'Close',exact:true}).click();
  checks.push('recovery: background export failure remains visible across routes without duplicate alerts');
  assert.deepEqual(errors,[]);
  await writeFile(output+'/studio-ui-result.json',JSON.stringify({passed:true,checks,pageErrors:errors},null,2));
  console.log('Studio UI checks passed: '+checks.join('; '));
}finally{await browser.close();await server.close();}
