import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { Badge, Button, Flex, Select, TextField } from '@radix-ui/themes';
import { api, errorText, mb, mmss, DEFAULT_RENDER_OPTIONS, type ClipMedia, type DemoMeta, type JobOutput, type RenderJob } from '../../api';
import { useStudio } from '../../StudioProvider';
import { Icon } from './Icon';
import { EmptyState } from './EmptyState';
import { ClipPlayer } from './ClipPlayer';
import { CurrentActivity } from './ActivityPanel';
import type { StudioRoute } from '../../studioState';
import { createMediaBroker } from '../../clipMediaRequests';
import { useRenderSource } from './useStudioAction';
import { isReviewPreview } from '../../renderPurpose';
import { demoDate } from '../../demoDate';
import { format } from 'date-fns';
import { FeedbackButton } from './FeedbackDialog';

export interface ClipEntry {key:string;job:RenderJob;output:JobOutput;demo?:DemoMeta;category:string;playerId?:string;playerName?:string;tags:string[];matchName?:string}
export function clipEntries(jobs:RenderJob[],demos:DemoMeta[]):ClipEntry[] {
  const byId = new Map(demos.map(demo=>[demo.id,demo]));
  return jobs.flatMap(job=>{
    if(isReviewPreview(job))return [];
    const demo=byId.get(job.demoId);
    const byHighlight=new Map((job.sourceSnapshot?.highlights??job.analysisClips?.highlights??[]).map(highlight=>[highlight.id,highlight]));
    const momentCategory=(highlight:ReturnType<typeof byHighlight.get>)=>!highlight||!Array.isArray(highlight.tags)?'other':highlight.tags.includes('death')?'death':highlight.tags.includes('whiff_candidate')?'whiff_candidate':highlight.tags.some(tag=>['whiff','extreme_whiff'].includes(tag))?'whiff':highlight.tags.some(tag=>['exceptional_shot','exceptional-shot'].includes(tag))?'shot':highlight.tags.some(tag=>['blooper','funny','ai'].includes(tag))?'blooper':job.analysisClips||highlight.tags.includes('custom')?'custom':'highlight';
    const selected=[...new Set(job.highlightIds)].map(id=>byHighlight.get(id));
    const complete=selected.length>0&&selected.every(highlight=>!!highlight);
    const categories=new Set(selected.map(momentCategory));
    const mergedCategory=!complete||categories.has('other')?'other':categories.size===1?[...categories][0]!:'mixed';
    const players=new Set(selected.map(highlight=>highlight?.player?.steamid));
    const mergedPlayer=complete&&players.size===1&&selected.every(highlight=>!!highlight?.player?.steamid)?selected[0]?.player:undefined;
    const mergedTags=complete?[...new Set(selected.flatMap(highlight=>Array.isArray(highlight?.tags)?highlight.tags:[]))]:[];
    return job.outputs.map(output=>{
      const highlight=byHighlight.get(output.highlightId??'');
      const tags=output.isFinal?mergedTags:highlight?.tags??[];
      const category=output.isFinal?mergedCategory:momentCategory(highlight);
      const player=output.isFinal?mergedPlayer:highlight?.player;
      return {key:job.id+'|'+output.file,job,output,demo,category,playerId:player?.steamid,playerName:player?.name,tags,matchName:demo?.mapName??job.sourceSnapshot?.demoInfo.mapName};
    });
  });
}
const categoryLabels:Record<string,{key:string;label:string}>={all:{key:'categoryAll',label:'All moments'},highlight:{key:'categoryHighlight',label:'Highlight'},death:{key:'categoryDeath',label:'Death'},blooper:{key:'categoryBlooper',label:'Blooper'},whiff:{key:'categoryWhiff',label:'Whiff'},whiff_candidate:{key:'categoryWhiffCandidate',label:'Possible whiff'},shot:{key:'categoryExceptionalShot',label:'Exceptional shot'},custom:{key:'categoryCustom',label:'Custom'},mixed:{key:'categoryMixed',label:'Mixed'},other:{key:'categoryOther',label:'Older / uncategorized'}};
function categoryLabel(category:string,t:TFunction){const value=categoryLabels[category]??categoryLabels.other!;return t(`studio.${value.key}`,{defaultValue:value.label});}
const savedClipFilters={query:'',kind:'all',match:'all',player:'all'};

const mediaRequests=createMediaBroker(api.clipMedia,api.cancelClipMedia);
export function useClipMedia(entry:ClipEntry|undefined,visible:boolean) {
  const [media,setMedia]=useState<ClipMedia>();
  const [error,setError]=useState<string>();
  useEffect(()=>{
    setMedia(undefined);setError(undefined);
    if(!entry||!visible)return;
    let alive=true;
    const request=mediaRequests.acquire(entry.job.id,entry.output.file);
    void request.promise.then(value=>{if(alive)setMedia(value);}).catch(e=>{if(alive)setError(errorText(e));});
    return()=>{alive=false;request.release();};
  },[entry?.key,entry?.output.bytes,visible]);
  return {media,error};
}
export function ClipCard({entry,onNavigate,featured=false}: {entry:ClipEntry;onNavigate:(route:StudioRoute)=>void;featured?:boolean}) {
  const {t}=useTranslation();
  const ref=useRef<HTMLButtonElement>(null);
  const [visible,setVisible]=useState(false);
  useEffect(()=>{if(!ref.current)return;const observer=new IntersectionObserver(items=>setVisible(items.some(item=>item.isIntersecting)));observer.observe(ref.current);return()=>observer.disconnect();},[]);
  const {media,error}=useClipMedia(entry,visible);
  return <button ref={ref} className={'clip-card '+(featured?'clip-featured':'')} onClick={()=>onNavigate({page:'clips',clipKey:entry.key})}>
    <div className="clip-poster">{media?.posterPath?<img src={api.fileSrc(media.posterPath)} alt="" loading="lazy"/>:<div className="clip-placeholder"><Icon name="clips" size={featured?56:32}/><span>{error?t('studio.previewUnavailable', { defaultValue: "Preview unavailable" }):entry.matchName||t('studio.yourGameplay', { defaultValue: "Your gameplay" })}</span></div>}
      <span className="clip-play"><Icon name="play" size={24}/></span><div className="clip-poster-meta">{media&&<span>{mmss(media.durationSeconds)}</span>}<span>{media?media.height+'p · '+Math.round(media.fps)+' FPS':t('studio.requestedHeight',{defaultValue:'Requested {{height}}p',height:entry.job.options.height})}</span></div>
    </div><div className="clip-card-copy"><span className="clip-eyebrow">{entry.matchName||'CS2'} · {categoryLabel(entry.category,t)}</span><strong>{entry.output.title}</strong><span>{entry.playerName||t('studio.playerInfoUnavailable', { defaultValue: "Player info unavailable" })} · {mb(entry.output.bytes)}</span></div>
  </button>;
}
export function ClipLibrary({clipKey,onNavigate}: {clipKey?:string;onNavigate:(route:StudioRoute)=>void}) {
  const {t}=useTranslation();
  const state=useStudio();
  const entries=useMemo(()=>clipEntries(state.jobs,state.demos),[state.jobs,state.demos]);
  const [query,setQuery]=useState(savedClipFilters.query);
  const [kind,setKind]=useState(savedClipFilters.kind);
  const [match,setMatch]=useState(savedClipFilters.match);
  const [player,setPlayer]=useState(savedClipFilters.player);
  const source=useRenderSource(onNavigate);
  useEffect(()=>{Object.assign(savedClipFilters,{query,kind,match,player});},[query,kind,match,player]);
  const matchSources=[...new Map(entries.map(entry=>[entry.job.demoId,entry])).entries()];
  const matchLabels=matchSources.map(([id,entry])=>{
    const timestamp=entry.demo?demoDate(entry.demo):undefined;
    const date=typeof timestamp==='number'&&Number.isFinite(timestamp)?format(timestamp,'yyyy-MM-dd HH:mm'):undefined;
    const name=entry.demo?.name??entry.job.sourceSnapshot?.demoPath.split(/[/\\]/).pop();
    return {id,label:[entry.matchName??t('studio.sourceMetadataUnavailable',{defaultValue:'Source metadata unavailable'}),date,name].filter(Boolean).join(' · ')};
  });
  const labelCounts=new Map<string,number>(),shortCounts=new Map<string,number>();
  for(const {id,label} of matchLabels){labelCounts.set(label,(labelCounts.get(label)??0)+1);const key=label+'\0'+id.slice(0,8);shortCounts.set(key,(shortCounts.get(key)??0)+1);}
  const matches=matchLabels.map(({id,label})=>[id,(labelCounts.get(label)??0)>1?`${label} · ${(shortCounts.get(label+'\0'+id.slice(0,8))??0)>1?id:id.slice(0,8)}`:label] as const);
  const selected=entries.find(entry=>entry.key===clipKey);
  const {media,error}=useClipMedia(selected,!!selected);
  useEffect(()=>{
    // An empty initial snapshot is still loading; it cannot invalidate session choices.
    if(!state.status)return;
    if(match!=='all'&&!entries.some(entry=>entry.job.demoId===match))setMatch('all');
    if(player!=='all'&&!entries.some(entry=>entry.playerId===player))setPlayer('all');
  },[state.status,entries,match,player]);
  const filtered=entries.filter(entry=>(kind==='all'||entry.category===kind)&&(match==='all'||entry.job.demoId===match)&&(player==='all'||entry.playerId===player)&&[entry.output.title,entry.matchName,entry.playerName].join(' ').toLowerCase().includes(query.toLowerCase()));
  const scroller=useRef<HTMLDivElement>(null);
  const [columns,setColumns]=useState(3);
  useEffect(()=>{const el=scroller.current;if(!el)return;const measure=()=>{if(el.clientWidth>0)setColumns(Math.max(1,Math.floor((el.clientWidth+20)/320)));};measure();const observer=new ResizeObserver(measure);observer.observe(el);return()=>observer.disconnect();},[selected?.key,entries.length,filtered.length>0]);
  const rows=Math.ceil(filtered.length/columns);
  const virtual=useVirtualizer({count:rows,getScrollElement:()=>scroller.current,estimateSize:()=>310,overscan:2,useAnimationFrameWithResizeObserver:true});
  if(selected)return <div className="studio-page"><Button variant="ghost" onClick={()=>onNavigate({page:'clips'})}><Icon name="arrowLeft"/>{t('studio.allClips', { defaultValue: "All clips" })}</Button>
    <div className="clip-detail-stage"><ClipPlayer src={api.fileSrc(selected.output.file)} title={selected.output.title} poster={media?.posterPath?api.fileSrc(media.posterPath):undefined}/></div>
    <div className="studio-section-heading"><div><span className="studio-eyebrow">{selected.matchName||t('studio.yourMatch', { defaultValue: "Your match" })}</span><h2>{selected.output.title}</h2><p>{media?media.width+' × '+media.height+' · '+Math.round(media.fps)+' FPS · '+mmss(media.durationSeconds):t('studio.requestedClipFormat',{defaultValue:'Requested {{width}} × {{height}} · {{fps}} FPS',width:selected.job.options.width,height:selected.job.options.height,fps:selected.job.options.fps})} · {mb(selected.output.bytes)}</p>{error&&<p className="studio-error">{error}</p>}</div>
    <Flex gap="2" wrap="wrap"><Button variant="soft" disabled={source.pending.has(selected.job.id)} onClick={()=>void source.open(selected.job)}>{source.pending.has(selected.job.id)?t('studio.openingSource', { defaultValue: "Opening source…" }):t('studio.openSourceMatch', { defaultValue: "Open source match" })}</Button><Button variant="outline" onClick={()=>void state.run(()=>api.reveal(selected.output.file))}><Icon name="folder"/>{t('studio.showInFolder', { defaultValue: "Show in folder" })}</Button><Button variant="outline" onClick={()=>void state.run(()=>api.open(selected.output.file))}>{t('studio.openInPlayer', { defaultValue: "Open in player" })}</Button></Flex></div>
    <section className="studio-card"><h3>{t('studio.recordingDetails', { defaultValue: "Recording details" })}</h3><p>{categoryLabel(selected.category,t)+' · '+(selected.playerName||t('studio.playerMetadataUnavailable', { defaultValue: "Player metadata unavailable for this older recording" }))}</p><Flex gap="2" wrap="wrap">{selected.tags.map(tag=><Badge key={tag}>{tag}</Badge>)}<Badge>{!selected.job.options.hud?t('studio.hudHidden', { defaultValue: "HUD hidden" }):(['hud','crosshair','radar','killFeed','chat'] as const).every(key=>selected.job.options[key])?t('studio.fullHudShort', { defaultValue: "Full HUD" }):t('studio.customHud', { defaultValue: "Custom HUD" })}</Badge><Badge>{selected.job.options.voice?t('studio.voiceEnabledRecorded', { defaultValue: "Voice enabled, if recorded" }):t('studio.voiceDisabled', { defaultValue: "Voice disabled" })}</Badge></Flex><Flex mt="4"><FeedbackButton source={{sourceKind:'clip',demoId:selected.job.demoId,jobId:selected.job.id,outputKey:selected.output.file,momentId:selected.output.highlightId,playerId:selected.playerId,originalProposal:{title:selected.output.title,tags:selected.tags,source:'registered-export'}}}/></Flex></section>
  </div>;
  return <div className="studio-page clip-library-page"><div className="studio-section-heading"><div><h2>{t('studio.clipCollectionTitle', { defaultValue: "Your clip collection" })}</h2><p>{t('studio.savedRecordingsDescription',{defaultValue:'{{count}} saved recordings. Highlights, deaths and the moments worth keeping.',count:entries.length})}</p></div><Button onClick={()=>onNavigate({page:'matches'})}>{t('studio.makeClip', { defaultValue: "Make a clip" })}</Button></div>
    <Flex gap="3" wrap="wrap" className="studio-filter-bar"><TextField.Root aria-label={t('studio.searchClips', { defaultValue: "Search clips" })} placeholder={t('studio.searchClipsPlaceholder', { defaultValue: "Search clips, maps or players…" })} value={query} onChange={e=>setQuery(e.target.value)}><TextField.Slot><Icon name="search"/></TextField.Slot></TextField.Root>
    <Select.Root value={kind} onValueChange={setKind}><Select.Trigger aria-label={t('studio.clipCategory', { defaultValue: "Clip category" })}/><Select.Content>{['all','highlight','death','blooper','whiff','whiff_candidate','shot','custom','mixed','other'].map(value=><Select.Item key={value} value={value}>{categoryLabel(value,t)}</Select.Item>)}</Select.Content></Select.Root>
    <Select.Root value={match} onValueChange={setMatch}><Select.Trigger className="studio-match-filter" aria-label={t('studio.filterClipsMatch', { defaultValue: "Filter clips by match" })}/><Select.Content className="studio-match-filter-menu"><Select.Item value="all">{t('studio.allMatches', { defaultValue: "All matches" })}</Select.Item>{matches.map(([id,name])=><Select.Item key={id} value={id}>{name}</Select.Item>)}</Select.Content></Select.Root>
    <Select.Root value={player} onValueChange={setPlayer}><Select.Trigger aria-label={t('studio.filterClipsPlayer', { defaultValue: "Filter clips by player" })}/><Select.Content><Select.Item value="all">{t('studio.allPlayers', { defaultValue: "All players" })}</Select.Item>{[...new Map(entries.filter(e=>e.playerId).map(e=>[e.playerId!,e.playerName||e.playerId!])).entries()].map(([id,name])=><Select.Item key={id} value={id}>{name}</Select.Item>)}</Select.Content></Select.Root></Flex>
    {filtered.length===0?<EmptyState variant={entries.length?'search':'clips'} title={entries.length?t('studio.noFilteredClips', { defaultValue: "No clips match these filters" }):t('studio.firstClipsTitle', { defaultValue: "Your best moments belong here" })} description={entries.length?t('studio.tryClipFilters', { defaultValue: "Try another map, category or search." }):t('studio.makeFirstClipDescription', { defaultValue: "Open a match, choose your moments and export your first clip." })} action={<Button onClick={()=>onNavigate({page:'matches'})}>{t('studio.browseMatches', { defaultValue: "Browse matches" })}</Button>}/>:<div ref={scroller} className="clip-gallery-scroll" style={{flex:1,minHeight:120,overflow:'auto'}}><div style={{height:virtual.getTotalSize(),position:'relative'}}>{virtual.getVirtualItems().map(row=><div key={row.key} data-index={row.index} ref={virtual.measureElement} className="clip-grid" style={{position:'absolute',top:0,left:0,width:'100%',gridTemplateColumns:'repeat('+columns+', minmax(0,1fr))',transform:'translateY('+row.start+'px)'}}>{filtered.slice(row.index*columns,(row.index+1)*columns).map(entry=><ClipCard key={entry.key} entry={entry} onNavigate={onNavigate}/>)}</div>)}</div></div>}
  </div>;
}
export function HomePage({onNavigate,onImport,onSync}: {onNavigate:(route:StudioRoute)=>void;onImport:()=>void;onSync:()=>void}) {
  const {t}=useTranslation();
  const state=useStudio();
  const entries=useMemo(()=>clipEntries(state.jobs,state.demos),[state.jobs,state.demos]);
  const downloaded=state.automation?.matches?.filter(match=>match.path).length??0;
  const defaults=state.settings?.settings.renderDefaults??DEFAULT_RENDER_OPTIONS;
  const matchHistoryAvailable=state.automation?.capabilities?.matchHistory!==false;
  return <div className="studio-page home-page"><section className="studio-welcome"><div><span className="studio-eyebrow">{t('studio.yourGameplayEyebrow', { defaultValue: "YOUR GAMEPLAY. YOUR STUDIO." })}</span><h1>{t('studio.homeTitleFirst', { defaultValue: "Make every moment" })}<br/>{t('studio.homeTitleSecond', { defaultValue: "worth keeping." })}</h1><p>{t('studio.homeDescriptionFirst', { defaultValue: "Great plays, rough deaths, unexpected laughs." })}<br/>{t('studio.homeDescriptionSecond', { defaultValue: "Turn your CS2 matches into clips on your own PC." })}</p><Flex gap="3" wrap="wrap">{matchHistoryAvailable&&<Button onClick={onSync} disabled={state.automation?.activity.busy}><Icon name="download"/>{t('studio.downloadMatches', { defaultValue: "Download matches" })}</Button>}<Button variant={matchHistoryAvailable?'outline':'solid'} onClick={onImport}><Icon name="import"/>{t('studio.importDemo', { defaultValue: "Import demo" })}</Button></Flex>{!matchHistoryAvailable&&<p className="studio-caption" role="status">{t('studio.manualImportNotice',{defaultValue:'Match downloads are unavailable in this installation. Import a saved CS2 demo to find and export your moments.'})}</p>}</div><div className="studio-welcome-stats"><strong>{entries.length}</strong><span>{t('studio.savedClips', { defaultValue: "saved clips" })}</span><strong>{downloaded||state.demos.length}</strong><span>{t('studio.studioMatches', { defaultValue: "matches in your studio" })}</span><Badge color={state.automation?.settings?.enabled?'green':'gray'}>{state.automation?.settings?.enabled?t('studio.automationOn', { defaultValue: "Automation on" }):t('studio.automationOff', { defaultValue: "Automation off" })}</Badge></div></section>
    <div className="home-feature-row"><div>{entries[0]?<ClipCard entry={entries[0]} onNavigate={onNavigate} featured/>:<EmptyState variant="clips" title="Your first clip starts with a match" description={matchHistoryAvailable?t('studio.firstClipDescription',{defaultValue:'Download your recent matches or import a saved demo.'}):t('studio.firstImportedClipDescription',{defaultValue:'Import a saved demo, then choose the moments you want to keep.'})} action={<Button onClick={()=>onNavigate({page:'matches'})}>{t('studio.browseMatches', { defaultValue: "Browse matches" })}</Button>}/>}</div><div className="home-status-column"><CurrentActivity onNavigate={onNavigate}/><section className="studio-card"><span className="studio-eyebrow">{t('studio.localByDesign', { defaultValue: "LOCAL BY DESIGN" })}</span><h3>{t('studio.qualityControlTitle', { defaultValue: "Your quality. Your control." })}</h3><p>{t('studio.exportDefaultLocalDescription',{defaultValue:'Your export default: {{height}}p at {{fps}} FPS. Your recordings, models and files stay on your PC.',height:defaults.height,fps:defaults.fps})}</p><Button variant="ghost" onClick={()=>onNavigate({page:'settings',section:'recording'})}>{t('studio.recordingSettings', { defaultValue: "Recording settings" })} <Icon name="more"/></Button></section></div></div>
    {entries.length>0&&<><div className="studio-section-heading"><h2>{t('studio.recentlyForged', { defaultValue: "Recently forged" })}</h2><Button variant="ghost" onClick={()=>onNavigate({page:'clips'})}>{t('studio.viewAllClips', { defaultValue: "View all clips" })}</Button></div><div className="clip-grid">{entries.slice(0,6).map(entry=><ClipCard key={entry.key} entry={entry} onNavigate={onNavigate}/>)}</div></>}
    <div className="studio-section-heading"><h2>{t('studio.recentMatches', { defaultValue: "Recent matches" })}</h2><Button variant="ghost" onClick={()=>onNavigate({page:'matches'})}>{t('studio.viewAllMatches', { defaultValue: "View all matches" })}</Button></div><div className="studio-card">{state.demos.slice(0,3).map(demo=><button className="studio-match-row" key={demo.id} onClick={()=>onNavigate({page:'matches',demoId:demo.id,tab:'moments'})}><Icon name="matches"/><span><strong>{demo.mapName||demo.name}</strong><small>{new Date(demo.matchTimeMs??demo.createdMs).toLocaleDateString()}</small></span><Badge color={demo.status==='parsed'?'green':'amber'}>{demo.status==='parsed'?t('studio.readyToClip', { defaultValue: "Ready to clip" }):t(`demoList.status.${demo.status}`)}</Badge><Icon name="more"/></button>)}{state.demos.length===0&&<p>{matchHistoryAvailable?t('studio.noMatchesDescription', { defaultValue: "No matches yet. Download from Steam or import a demo to get started." }):t('studio.noImportedMatchesDescription',{defaultValue:'No matches yet. Import a saved demo to get started.'})}</p>}</div>
  </div>;
}
