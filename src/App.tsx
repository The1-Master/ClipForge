import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { open } from '@tauri-apps/plugin-dialog';
import { AlertDialog, Badge, Button, Flex } from '@radix-ui/themes';
import { api } from './api';
import { automationApi } from './api_automation';
import { NotificationProvider } from './components/Notifications';
import { StartupGate } from './components/StartupGate';
import { StudioProvider, useStudio } from './StudioProvider';
import { DemoList } from './components/DemoList';
import { DemoView } from './components/DemoView';
import { ToolInstallDialog } from './components/ToolInstallDialog';
import { Brand } from './components/studio/Brand';
import { Icon } from './components/studio/Icon';
import { EmptyState } from './components/studio/EmptyState';
import { HomePage, ClipLibrary } from './components/studio/ClipLibrary';
import { ActivityPage, CurrentActivity } from './components/studio/ActivityPanel';
import { StudioSettings } from './components/studio/StudioSettings';
import type { DraftGuard, StudioRoute } from './studioState';
import { APP_VERSION } from './appVersion';

export function App() {
  return <NotificationProvider><StartupGate><StudioProvider><StudioApp/></StudioProvider></StartupGate></NotificationProvider>;
}
const DESTINATIONS=[{id:'home',label:'Home',key:'navHome',icon:'home'},{id:'clips',label:'Clips',key:'navClips',icon:'clips'},{id:'matches',label:'Matches',key:'navMatches',icon:'matches'},{id:'activity',label:'Activity',key:'navActivity',icon:'activity'},{id:'settings',label:'Settings',key:'navSettings',icon:'settings'}] as const;
function StudioApp() {
  const {t}=useTranslation();
  const state=useStudio();
  const [route,setRoute]=useState<StudioRoute>({page:'home'});
  const routeRef=useRef(route);
  routeRef.current=route;
  const [menuOpen,setMenuOpen]=useState(false);
  const [selectionRequest,setSelectionRequest]=useState(0);
  const [pendingRoute,setPendingRoute]=useState<StudioRoute>();
  const pendingBack=useRef(false);
  const [leaving,setLeaving]=useState(false);
  const [settingsVisited,setSettingsVisited]=useState(false);
  const [toolsRequest,setToolsRequest]=useState(0);
  const [toolsTarget,setToolsTarget]=useState<'render'|'replay'>('render');
  const guards=useRef(new Map<string,DraftGuard>());
  const history=useRef<StudioRoute[]>([]);
  const lastMatch=useRef<StudioRoute>({page:'matches',tab:'moments'});
  const lastTabs=useRef(new Map<string,string>());
  const navRef=useRef<HTMLElement>(null);
  const menuRef=useRef<HTMLButtonElement>(null);
  useEffect(()=>{
    if(!menuOpen)return;
    const panel=navRef.current;
    const buttons=()=>[...(panel?.querySelectorAll<HTMLButtonElement>('button:not([disabled])')??[])].filter(button=>button.getBoundingClientRect().width>0);
    (panel?.querySelector<HTMLButtonElement>('button[aria-current=page]')??buttons()[0])?.focus();
    const keys=(event:KeyboardEvent)=>{
      if(event.key==='Escape'){event.preventDefault();setMenuOpen(false);}
      if(event.key==='Tab'){const items=buttons();const first=items[0],last=items.at(-1);if(event.shiftKey&&document.activeElement===first){event.preventDefault();last?.focus();}else if(!event.shiftKey&&document.activeElement===last){event.preventDefault();first?.focus();}}
    };
    window.addEventListener('keydown',keys);
    return()=>{window.removeEventListener('keydown',keys);menuRef.current?.focus();};
  },[menuOpen]);
  const selected=state.demos.find(d=>d.id===route.demoId);
  const onDraft=useCallback((key:string,guard:DraftGuard|undefined)=>{if(guard)guards.current.set(key,guard);else guards.current.delete(key);},[]);
  const commitRoute=useCallback((next:StudioRoute,remember=true)=>{
    const current=routeRef.current;
    if(remember&&JSON.stringify(current)!==JSON.stringify(next)){history.current.push(current);if(history.current.length>50)history.current.shift();}
    routeRef.current=next;setRoute(next);
    if(next.page==='settings')setSettingsVisited(true);
    if(next.page==='matches'&&next.demoId){lastMatch.current=next;setSelectionRequest(value=>value+1);}
    setMenuOpen(false);
  },[]);
  const navigate=useCallback((next:StudioRoute)=>{
    if(next.page==='matches'&&!next.demoId)next={...lastMatch.current,page:'matches'};
    if(next.page==='matches'&&next.demoId&&!next.tab)next={...next,tab:lastTabs.current.get(next.demoId)||'moments'};
    if(route.page==='settings'&&next.page!=='settings'&&[...guards.current.values()].some(guard=>guard.dirty)){pendingBack.current=false;setPendingRoute(next);return;}
    commitRoute(next);
  },[route.page,commitRoute]);
  const leave=async(save:boolean)=>{
    setLeaving(true);
    const dirty=[...guards.current.values()].filter(guard=>guard.dirty);
    let ok=true;
    if(save){for(const guard of dirty)if(!await guard.save()){ok=false;break;}}
    else dirty.forEach(guard=>guard.discard());
    setLeaving(false);
    if(ok&&pendingRoute){if(pendingBack.current)history.current.pop();commitRoute(pendingRoute,!pendingBack.current);pendingBack.current=false;setPendingRoute(undefined);}
    if(!ok)setPendingRoute(undefined);
  };
  useEffect(()=>{
    if(!state.status)return;
    const demos=new Set(state.demos.map(demo=>demo.id));
    const clips=new Set(state.jobs.flatMap(job=>job.outputs.map(output=>job.id+'|'+output.file)));
    const exists=(entry:StudioRoute)=>(entry.page!=='matches'||!entry.demoId||demos.has(entry.demoId))&&(entry.page!=='clips'||!entry.clipKey||clips.has(entry.clipKey));
    history.current=history.current.filter(exists);
    if(!exists(lastMatch.current))lastMatch.current={page:'matches',tab:'moments'};
    for(const id of lastTabs.current.keys())if(!demos.has(id))lastTabs.current.delete(id);
    if(!exists(route)){
      const replacement:StudioRoute={page:route.page};
      const sameLibrary=(entry:StudioRoute)=>entry.page===replacement.page&&(!entry.demoId&&!entry.clipKey);
      while(history.current.at(-1)&&sameLibrary(history.current.at(-1)!))history.current.pop();
      routeRef.current=replacement;
      setRoute(replacement);
    }
  },[route,state.demos,state.jobs,state.status]);
  const importDemo=()=>{void state.run(async()=>{
    const paths=await open({multiple:true,filters:[{name:'CS2 demo',extensions:['dem']}]});
    if(!paths)return;
    const files=Array.isArray(paths)?paths:[paths];
    let first:string|undefined;
    for(const path of files){const demo=await api.registerDemo(path);first??=demo.id;await api.parse(demo.id);}
    if(first){await state.rescan();navigate({page:'matches',demoId:first,tab:'moments'});}
  });};
  const matchHistoryAvailable=state.automation?.capabilities?.matchHistory!==false;
  const sync=()=>{if(!matchHistoryAvailable)return;void state.run(()=>automationApi.action({command:'sync'}));};
  const activeCount=state.jobs.filter(job=>job.status==='running'||job.status==='queued').length+state.analysisJobs.filter(job=>job.status==='running'||job.status==='queued').length+(state.automation?.activity.busy?1:0)+(state.community?.activity?.busy?1:0);
  const destinationLabel=(item:typeof DESTINATIONS[number])=>t(`studio.${item.key}`,{defaultValue:item.label});
  const title=route.page==='matches'&&selected?(selected.mapName||t('studio.matchStudio', { defaultValue: "Match studio" })):destinationLabel(DESTINATIONS.find(item=>item.id===route.page)!);
  const subtitle={home:t('studio.subtitleHome', { defaultValue: "Your moments, ready to forge." }),clips:t('studio.subtitleClips', { defaultValue: "Everything you have recorded, in one collection." }),matches:matchHistoryAvailable?t('studio.subtitleMatches', { defaultValue: "Choose a match. Find your moments." }):t('studio.manualImportHeader',{defaultValue:'Import a saved demo. Match downloads are unavailable here.'}),activity:t('studio.subtitleActivity', { defaultValue: "Know what is running and what happens next." }),settings:t('studio.subtitleSettings', { defaultValue: "Make ClipForge work your way." })}[route.page];
  const showTools=(target:'render'|'replay')=>{setToolsTarget(target);setToolsRequest(value=>value+1);navigate({page:'settings',section:'tools'});};
  return <div className={'studio-shell '+(menuOpen?'nav-open':'')}>
    <ToolInstallDialog setup={state.setup}/>
    <aside ref={navRef} className="studio-nav" role={menuOpen?'dialog':undefined} aria-modal={menuOpen||undefined} aria-label={menuOpen?'Navigation menu':undefined}><Brand/><nav aria-label="Main navigation">{DESTINATIONS.map(item=><button key={item.id} aria-label={destinationLabel(item)} aria-current={route.page===item.id?'page':undefined} className={route.page===item.id?'active':''} onClick={()=>navigate({page:item.id})}><Icon name={item.icon}/><span>{destinationLabel(item)}</span>{item.id==='activity'&&activeCount>0&&<Badge size="1" aria-hidden="true">{activeCount}</Badge>}</button>)}</nav>
    <footer className="studio-nav-footer"><div className="studio-local-mark"><Icon name="spark" size={18}/><span>{t('studio.localMade', { defaultValue: "Made on your PC" })}</span></div><small>ClipForge · {APP_VERSION}</small><Button variant="ghost" onClick={()=>navigate({page:'settings',section:'connection'})}>{!matchHistoryAvailable?t('studio.manualDemoImport',{defaultValue:'Manual demo import'}):state.automation?.settings?.authCodeSet?t('studio.steamConnected', { defaultValue: "Steam connected" }):t('studio.connectSteam', { defaultValue: "Connect Steam" })}</Button></footer></aside>
    {menuOpen&&<button className="studio-nav-scrim" aria-label="Close navigation menu" onClick={()=>setMenuOpen(false)}/>}
    <header className="studio-header"><Button ref={menuRef} variant="ghost" className="studio-menu-button" onClick={()=>setMenuOpen(value=>!value)} aria-expanded={menuOpen}><Icon name="menu"/>{t('studio.menu', { defaultValue: "Menu" })}</Button>
      <div className="studio-header-copy studio-header-title">{route.page==='matches'&&selected&&<div className="studio-breadcrumbs"><button onClick={()=>{lastMatch.current={page:'matches'};navigate({page:'matches'});}}>{t('studio.navMatches', { defaultValue: "Matches" })}</button><span>/ {t('studio.matchStudio', { defaultValue: "Match studio" })}</span></div>}<h1>{title}</h1><p>{subtitle}</p></div>
      <div className="studio-header-actions">{history.current.length>0&&<Button variant="ghost" aria-label="Go back" onClick={()=>{const previous=history.current.at(-1);if(previous){if(route.page==='settings'&&previous.page!=='settings'&&[...guards.current.values()].some(guard=>guard.dirty)){pendingBack.current=true;setPendingRoute(previous);}else{history.current.pop();commitRoute(previous,false);}}}}><Icon name="arrowLeft"/></Button>}
      {(route.page==='matches'||route.page==='clips')&&<Button variant="soft" onClick={importDemo}><Icon name="import"/>{t('studio.importDemo', { defaultValue: "Import demo" })}</Button>}
      {route.page==='matches'&&<Button onClick={sync} disabled={!matchHistoryAvailable||state.automation?.activity.busy}><Icon name="download"/>{t('studio.downloadMatches', { defaultValue: "Download matches" })}</Button>}</div>
    </header>
    <main className="studio-main" id="studio-content">
      {state.error&&<div className="studio-backend-error" role="alert"><span>{state.error}</span><Button variant="soft" onClick={()=>void state.refreshAutomation(true).then(state.refresh)}>{t('studio.reconnect', { defaultValue: "Reconnect" })}</Button></div>}
      {route.page==='home'&&<HomePage onNavigate={navigate} onImport={importDemo} onSync={sync}/>}
      {route.page==='clips'&&<ClipLibrary clipKey={route.clipKey} onNavigate={navigate}/>}
      {route.page==='activity'&&<ActivityPage onNavigate={navigate}/>}
      <div className="matches-workspace" hidden={route.page!=='matches'}>
        <aside className="match-library-panel"><DemoList active={route.page==='matches'} demos={state.demos} jobs={state.jobs} selectedId={route.demoId} toolbar={null} selectionRequest={selectionRequest} onSelect={id=>navigate({page:'matches',demoId:id,tab:lastTabs.current.get(id)||'moments'})} onChanged={state.refresh} onRefresh={state.rescan}/></aside>
        <div className="match-detail-panel">{route.page==='matches'&&!matchHistoryAvailable&&<p className="studio-caption" role="status">{t('studio.manualImportNotice',{defaultValue:'Match downloads are unavailable in this installation. Import a saved CS2 demo to find and export your moments.'})}</p>}{route.page==='matches'&&(selected?<DemoView key={selected.id} analysisJobs={state.analysisJobs} onSetup={showTools} tab={route.tab||'moments'} setTab={tab=>{lastTabs.current.set(selected.id,tab);setRoute(value=>({...value,tab}));lastMatch.current={page:'matches',demoId:selected.id,tab};}} meta={selected} jobs={state.jobs.filter(job=>job.demoId===selected.id)} status={state.status} onChanged={state.refresh} onRemoved={()=>{lastMatch.current={page:'matches'};setRoute({page:'matches'});}} playerId={state.automation?.settings?.steamId} review={state.automation?.review?.demoId===selected.id?state.automation.review:undefined} reviewBusy={state.automation?.activity.busy||state.automation?.review?.complete===false} onReviewMatch={(reviewPlayer,mode)=>void state.run(()=>automationApi.reviewMatch(selected.id,reviewPlayer,mode))}/>:<EmptyState variant="matches" title={t('studio.chooseMatchTitle', { defaultValue: "Choose a match to find your moments" })} description={matchHistoryAvailable?t('studio.chooseMatchDescription', { defaultValue: "Pick a match from the library, download from Steam or import a demo." }):t('studio.chooseImportedMatchDescription',{defaultValue:'Pick a match from the library or import a saved demo.'})} action={<Button onClick={importDemo}>{t('studio.importADemo', { defaultValue: "Import a demo" })}</Button>}/>)}</div>
      </div>
      {settingsVisited&&<div className="studio-settings-host" hidden={route.page!=='settings'}><StudioSettings active={route.page==='settings'} section={route.section} onNavigate={navigate} onDraft={onDraft} toolsRequest={toolsRequest} toolsTarget={toolsTarget}/></div>}
    </main>
    <footer className="studio-status"><CurrentActivity compact onNavigate={navigate}/></footer>
    <AlertDialog.Root open={!!pendingRoute} onOpenChange={value=>{if(!value&&!leaving)setPendingRoute(undefined);}}><AlertDialog.Content maxWidth="520px"><AlertDialog.Title>{t('studio.keepChangesTitle', { defaultValue: "Keep your settings changes?" })}</AlertDialog.Title><AlertDialog.Description>{t('studio.keepChangesDescription', { defaultValue: "You have unsaved edits. Save them before leaving, or discard them." })}</AlertDialog.Description><Flex gap="3" justify="end" mt="4" wrap="wrap"><Button variant="soft" disabled={leaving} onClick={()=>setPendingRoute(undefined)}>{t('studio.stayHere', { defaultValue: "Stay here" })}</Button><Button variant="outline" disabled={leaving} onClick={()=>void leave(false)}>{t('studio.discardAndLeave', { defaultValue: "Discard and leave" })}</Button><Button disabled={leaving} onClick={()=>void leave(true)}>{leaving?t('studio.saving', { defaultValue: "Saving…" }):t('studio.saveAndLeave', { defaultValue: "Save and leave" })}</Button></Flex></AlertDialog.Content></AlertDialog.Root>
  </div>;
}
