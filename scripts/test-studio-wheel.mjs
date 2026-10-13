import assert from 'node:assert/strict';
import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
const app='J:/CS2ClipStudio/app';
const require=createRequire(app+'/package.json');
const {createServer}=await import(pathToFileURL(require.resolve('vite')).href);
const {chromium}=require(process.env.PLAYWRIGHT_MODULE);
const output='J:/CS2ClipStudio/research/benchmarks/redesign/settings-wheel';
await mkdir(output,{recursive:true});
const server=await createServer({root:app,cacheDir:'J:/CS2ClipStudio/cache/settings-wheel-vite',server:{host:'127.0.0.1',port:0,watch:{ignored:['**/*']},hmr:false}});
await server.listen();
const browser=await chromium.launch({channel:'chrome',headless:true});
const base=await readFile(app+'/scripts/test-ui.mjs','utf8');
const begin=base.indexOf('  await page.addInitScript(() => {'),end=base.indexOf('  await page.goto(',begin);
const studio=await readFile(app+'/scripts/test-studio.mjs','utf8');
const studioBegin=studio.indexOf('  await page.addInitScript(()=>{'),studioEnd=studio.indexOf('  await page.goto(',studioBegin);
const AsyncFunction=Object.getPrototypeOf(async function(){}).constructor;
const cases=[],errors=[];
const suffix=process.env.WHEEL_CHECK_SUFFIX||'regression';
async function readState(page){return page.evaluate(()=>{
 const elements=['.studio-main','.studio-settings-host','.settings-shell','.settings-content','.settings-page','fieldset'].map(selector=>{const el=document.querySelector(selector);const r=el.getBoundingClientRect(),style=getComputedStyle(el);return{selector,top:r.top,bottom:r.bottom,height:r.height,clientHeight:el.clientHeight,scrollHeight:el.scrollHeight,scrollTop:el.scrollTop,display:style.display,overflowY:style.overflowY};});
 const main=document.querySelector('.studio-main').getBoundingClientRect();
 const controls=['Verify installation','Diagnostics','Source 2 Viewer CLI'].map(label=>{const el=[...document.querySelectorAll('.settings-page button,.settings-page input')].find(e=>(e.getAttribute('aria-label')||e.textContent.trim())===label);const r=el.getBoundingClientRect();const x=r.left+r.width/2,y=r.top+r.height/2;const hit=x>=0&&x<innerWidth&&y>=main.top&&y<Math.min(innerHeight,main.bottom)?document.elementFromPoint(x,y):null;return{label,top:r.top,bottom:r.bottom,centerInContent:y>=main.top&&y<main.bottom,actuallyReachable:!!hit&&(el===hit||el.contains(hit)),ancestorScroll:elements.filter(e=>e.scrollTop>0).map(e=>({selector:e.selector,scrollTop:e.scrollTop}))};});
 return{elements,controls};
});}
try{
 for(const viewport of [{width:1366,height:768},{width:960,height:640}])for(const font of ['small','large']){
  const context=await browser.newContext({viewport});const page=await context.newPage();page.setDefaultTimeout(6000);page.on('pageerror',e=>errors.push(e.message));
  await new AsyncFunction('page',base.slice(begin,end).replace("localStorage.setItem('demodesk.appearance', 'dark');",`localStorage.setItem('demodesk.appearance','dark');localStorage.setItem('demodesk.fontSize','${font}');`))(page);
  await new AsyncFunction('page',studio.slice(studioBegin,studioEnd))(page);
  await page.addInitScript(()=>{window.toolPaths={steamDir:'J:/synthetic/Steam',cs2Dir:'J:/synthetic/CS2',cs2Exe:'J:/synthetic/cs2.exe',hlaeExe:'J:/synthetic/hlae.exe',hlaeDll:'J:/synthetic/afx.dll',ffmpegExe:'J:/synthetic/ffmpeg.exe',vrfExe:'J:/synthetic/Source2Viewer-CLI.exe'};});
  await page.goto(server.resolvedUrls.local[0]);
  await page.getByRole('navigation',{name:'Main navigation'}).getByRole('button',{name:'Settings',exact:true}).click();
  await page.getByRole('navigation',{name:'Settings sections'}).getByRole('button',{name:/^Tools /}).click();
  await page.getByRole('heading',{name:'Tools and installation',exact:true}).waitFor();
  const before=await readState(page);const result={viewport,font,before,wheels:[],seen:new Set()};
  before.controls.filter(c=>c.actuallyReachable).forEach(c=>result.seen.add(c.label));
  const main=await page.locator('.studio-main').boundingBox();
  const point={x:main.x+main.width-60,y:main.y+main.height*.6};
  await page.mouse.move(point.x,point.y);
  for(let i=0;i<12;i++){
   await page.mouse.wheel(0,400);await page.waitForTimeout(70);
   const state=await readState(page);state.controls.filter(c=>c.actuallyReachable).forEach(c=>result.seen.add(c.label));result.wheels.push(state);
   if(result.seen.size===3)break;
  }
  result.seen=[...result.seen];result.passed=result.seen.length===3;
  result.after=await readState(page);result.wheelPoint=point;
  await page.screenshot({path:output+'/'+suffix+'-'+viewport.width+'-'+font+'.png'});
  cases.push(result);console.log(JSON.stringify({viewport,font,passed:result.passed,seen:result.seen,before:before.elements,after:result.after.elements}));
  await context.close();
 }
}finally{
 const report={passed:cases.length===4&&cases.every(c=>c.passed)&&errors.length===0,method:'Synthetic current App fixture; ordinary mouse wheel over right-side Tools form content. No Playwright auto-scroll after opening Tools, native user data or game operations.',cases,errors};
 await writeFile(output+'/'+suffix+'-result.json',JSON.stringify(report,null,2));await browser.close();await server.close();
 console.log(JSON.stringify({passed:report.passed,report:output+'/'+suffix+'-result.json'}));if(!report.passed)process.exitCode=1;
}
