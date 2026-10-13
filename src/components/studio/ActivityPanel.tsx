import { useTranslation } from 'react-i18next';
import i18n from '../../i18n';
import { useEffect, useState } from 'react';
import { Badge, Button, Flex, Progress } from '@radix-ui/themes';
import { api, type RenderJob } from '../../api';
import { automationApi } from '../../api_automation';
import { useStudio } from '../../StudioProvider';
import { Icon } from './Icon';
import type { StudioRoute } from '../../studioState';
import { useRenderSource, useStudioAction } from './useStudioAction';
import { isReviewPreview } from '../../renderPurpose';
import { CommunityTasks } from './CommunitySettings';

export function activitySummary(state: ReturnType<typeof useStudio>) {
  const t=i18n.getFixedT(i18n.language);
  const cancelling = state.jobs.find(job => ['running','queued'].includes(job.status) && job.stage === 'cancelling');
  if (cancelling) return {title:t('studio.finishingCancellation', { defaultValue: "Finishing cancellation" }),detail:t('studio.cleanupWaiting', { defaultValue: "Waiting for recording and encoding cleanup to finish." }),running:true,render:cancelling};
  const render = state.jobs.find(job => job.status === 'running');
  const review = state.automation?.review;
  const work = state.automation?.activity;
  if (render) return {title:isReviewPreview(render)?t('studio.reviewPreview',{defaultValue:'Review preview'}):t('studio.makingClip', { defaultValue: "Making your clip" }),detail:render.stage ?? t('studio.preparingRecording', { defaultValue: "Preparing recording" }),running:true,render};
  if (work?.busy) return {title:work.task === 'review' ? t('studio.reviewingGameplay', { defaultValue: "Reviewing your gameplay" }) : work.task === 'install-models' ? t('studio.installingLocalAi', { defaultValue: "Installing local AI" }) : t('studio.findingMatches', { defaultValue: "Finding your matches" }),detail:work.progress ?? t('studio.working', { defaultValue: "Working…" }),running:true};
  if (review?.complete === false) return {title:t('studio.aiBlooperReview', { defaultValue: "AI blooper review" }),detail:review.status,running:true};
  const parsing = state.demos.find(demo => ['queued','validating','parsing'].includes(demo.status));
  if (parsing) return {title:t('studio.readingMatch', { defaultValue: "Reading your match" }),detail:parsing.mapName || parsing.name,running:true};
  const analysis = state.analysisJobs.find(job => job.status === 'running');
  if (analysis?.phase === 'waiting-for-compute') return {title:t('studio.waitingProcessingSlot',{defaultValue:'Waiting for the processing slot'}),detail:t('studio.otherTaskUsesResources',{defaultValue:'Another task is using the recording or AI resources.'}),running:false};
  if (analysis) return {title:t('studio.gameplayAnalysis', { defaultValue: "Gameplay analysis" }),detail:analysis.step ? t('studio.analysisStep',{defaultValue:'Step {{step}} of 3',step:analysis.step}) : t('studio.preparingAnalysis', { defaultValue: "Preparing analysis" }),running:true};
  const tool = Object.entries(state.setup).find(([,value]) => value?.running);
  if (tool) return {title:t('studio.installingTool',{defaultValue:'Installing {{tool}}',tool:tool[0]}),detail:tool[1]?.progress ?? t('studio.downloadingTool', { defaultValue: "Downloading required tool" }),running:true};
  if (state.community?.activity?.busy) return {title:state.community.activity.cancelling?'Cancelling community task…':'Community learning',detail:state.community.activity.progress??state.community.activity.phase??'Working…',running:true};
  if (state.community?.activity?.error) return {title:t('studio.needsAttention',{defaultValue:'Needs attention'}),detail:state.community.activity.error,running:false};
  const waiting = state.jobs.some(job => job.status === 'queued');
  if (state.automation?.queuePaused && waiting) return {title:t('studio.exportQueuePaused', { defaultValue: "Export queue paused" }),detail:t('studio.pauseQueueDescription', { defaultValue: "The current export finishes; subsequent exports wait." }),running:false};
  if (waiting && state.automation?.gamePresence === 'external') return {title:t('studio.waitingForCs2', { defaultValue: "Waiting for CS2 to close" }),detail:t('studio.gameSessionPriority', { defaultValue: "Your game session has priority." }),running:false};
  if (waiting) return {title:state.jobs.filter(job=>job.status==='queued').every(isReviewPreview)?t('studio.reviewPreviewQueued',{defaultValue:'Review preview queued'}):t('studio.clipsQueued', { defaultValue: "Clips queued" }),detail:t('studio.waitingRecordingSlot', { defaultValue: "Waiting for the recording slot." }),running:false};
  if (work?.phase==='waiting-for-game') return {title:t('studio.waitingForCs2', { defaultValue: "Waiting for CS2 to close" }),detail:work.progress??t('studio.retryAfterGame', { defaultValue: "Retry this task after your game session ends." }),running:false};
  if (work?.phase==='waiting-for-compute') return {title:t('studio.waitingProcessingSlot', { defaultValue: "Waiting for the processing slot" }),detail:work.progress??t('studio.otherTaskUsesResources', { defaultValue: "Another task is using the recording or AI resources." }),running:false};
  if (!state.automation) return {title:t('studio.checkingStudio', { defaultValue: "Checking your studio" }),detail:t('studio.loadingActivity', { defaultValue: "Loading local activity status…" }),running:false};
  if (state.automation.activity.error) return {title:t('studio.needsAttention', { defaultValue: "Needs attention" }),detail:state.automation.activity.error,running:false};
  return {title:t('studio.readyWhenYouAre', { defaultValue: "Ready when you are" }),detail:state.automation.settings?.enabled ? t('studio.autoProcessingOn', { defaultValue: "Automatic processing is on." }) : t('studio.autoProcessingOffDescription', { defaultValue: "Automatic processing is off. Choose a match or download new ones." }),running:false};
}
export function CurrentActivity({compact=false,onNavigate}: {compact?:boolean;onNavigate:(route:StudioRoute)=>void}) {
  const {t}=useTranslation();
  const state = useStudio();
  const summary = activitySummary(state);
  const work=state.automation?.activity;
  const review=state.automation?.review;
  const communityWork=state.community?.activity;
  const started=summary.render?.startedAt||(work?.busy?work.startedAt:review?.complete===false?review.startedAt:communityWork?.busy?communityWork.startedAt:undefined);
  const [,tick]=useState(0);
  useEffect(()=>{if(!started||!summary.running)return;const timer=setInterval(()=>tick(value=>value+1),1000);return()=>clearInterval(timer);},[started,summary.running]);
  const startedMs=typeof started==='number'?(started<1e12?started*1000:started):started?Date.parse(started):NaN;
  const elapsed=Number.isFinite(startedMs)?Math.max(0,Math.floor((Date.now()-startedMs)/1000)):undefined;
  const rawCompleted=summary.render?.completed??(work?.busy?work.completed:review?.complete===false?review.completed:communityWork?.busy?communityWork.completed:undefined);
  const rawTotal=summary.render?.total??(work?.busy?work.total:review?.complete===false?review.total:communityWork?.busy?communityWork.total:undefined);
  const completed=typeof rawCompleted==='number'&&Number.isFinite(rawCompleted)?rawCompleted:undefined;
  const total=typeof rawTotal==='number'&&Number.isFinite(rawTotal)?rawTotal:undefined;
  return <section className={compact?'studio-status-inner':'studio-card activity-card'} aria-label={t('studio.currentActivity', { defaultValue: "Current activity" })}>
    <span className={'activity-beacon '+(summary.running?'is-running':'')}><Icon name={summary.running?'activity':'check'} size={compact?18:24}/></span>
    <div className="activity-copy"><strong role="status" aria-live="polite">{summary.title}</strong><p>{summary.detail}</p>{!compact&&(elapsed!==undefined||completed!==undefined)&&<small>{completed!==undefined?t(total!==undefined?'studio.completedCountTotal':'studio.completedCount',{defaultValue:total!==undefined?'{{completed}} of {{total}} completed · ':'{{completed}} completed · ',completed,total}):''}{elapsed!==undefined?t('studio.elapsedTime',{defaultValue:'{{minutes}}m {{seconds}}s elapsed',minutes:Math.floor(elapsed/60),seconds:elapsed%60}):''}</small>}</div>
    {!compact && summary.render?.progress != null && <Progress value={Math.min(100,Math.max(0,summary.render.progress*100))} aria-label={t('studio.exportProgress', { defaultValue: "Export progress" })}/>}
    <Button size="1" variant="ghost" onClick={()=>onNavigate({page:'activity'})}>{t('studio.viewActivity', { defaultValue: "View activity" })}</Button>
  </section>;
}
const savedActivity={filter:'active',page:0};
const PAGE_SIZE=50;
function TaskLog({lines}:{lines:string[]}) {
  const {t}=useTranslation();
  const [open,setOpen]=useState(false);
  return <details className="studio-log-details" onToggle={event=>setOpen(event.currentTarget.open)}><summary>{t('studio.technicalLog', { defaultValue: "Technical log" })}</summary>{open&&<pre className="log">{lines.join('\n')}</pre>}</details>;
}
export function ActivityPage({onNavigate}: {onNavigate:(route:StudioRoute)=>void}) {
  const {t}=useTranslation();
  const state=useStudio();
  const [cancelling,setCancelling]=useState(new Set<string>());
  const [filter,setFilter]=useState(savedActivity.filter);
  const [page,setPage]=useState(savedActivity.page);
  const actions=useStudioAction();
  const source=useRenderSource(onNavigate);
  useEffect(()=>{Object.assign(savedActivity,{filter,page});},[filter,page]);
  useEffect(()=>{setCancelling(current=>new Set([...current].filter(id=>state.jobs.some(job=>job.id===id&&['running','queued'].includes(job.status)))));},[state.jobs]);
  const cancel=(job:RenderJob)=>actions.run('cancel:'+job.id,async()=>{
    setCancelling(value=>new Set(value).add(job.id));
    const ok=await state.run(()=>api.cancel(job.id));
    if(!ok)setCancelling(value=>{const next=new Set(value);next.delete(job.id);return next;});
  });
  const active=state.jobs.filter(job=>job.status==='running'||job.status==='queued');
  const list=filter==='active'?active:state.jobs;
  const pages=Math.max(1,Math.ceil(list.length/PAGE_SIZE));
  const currentPage=Math.min(page,pages-1);
  const pageJobs=list.slice(currentPage*PAGE_SIZE,(currentPage+1)*PAGE_SIZE);
  const demosById=new Map(state.demos.map(demo=>[demo.id,demo]));
  useEffect(()=>{if(page>=pages)setPage(pages-1);},[pages,page]);
  const a=state.automation;
  const review=a?.review;
  return <div className="studio-page"><CurrentActivity onNavigate={onNavigate}/>
    <CommunityTasks onOpen={()=>onNavigate({page:'settings',section:'community'})}/>
    <div className="studio-section-heading"><div><h2>{t('studio.processingQueue', { defaultValue: "Your processing queue" })}</h2><p>{t('studio.tasksKeepRunning', { defaultValue: "Tasks keep running when you move around the app." })}</p></div><Flex gap="2" wrap="wrap">
      <Button variant={filter==='active'?'solid':'soft'} onClick={()=>{setFilter('active');setPage(0);}}>{t('studio.activeTasks', { defaultValue: "Active" })}</Button><Button variant={filter==='all'?'solid':'soft'} onClick={()=>{setFilter('all');setPage(0);}}>{t('studio.taskHistory', { defaultValue: "History" })}</Button>
      <Button variant="outline" disabled={!a} onClick={()=>void state.run(()=>automationApi.pause(!a?.queuePaused))}>{a?.queuePaused?t('studio.resumeNextExports', { defaultValue: "Resume next exports" }):t('studio.pauseNextExports', { defaultValue: "Pause next exports" })}</Button>
    </Flex></div>
    {(a?.activity.busy||review?.complete===false)&&<section className="studio-card"><Flex justify="between" align="center" gap="3"><div><h3>{review?.complete===false?t('studio.aiBlooperReview', { defaultValue: "AI blooper review" }):t('studio.matchHistoryLocalAi',{defaultValue:'Match history / local AI'})}</h3><p>{a?.activity.progress||review?.status}</p></div><Button color="red" variant="soft" disabled={a?.activity.cancelling||review?.cancelling} onClick={()=>void state.run(automationApi.cancel)}>{a?.activity.cancelling||review?.cancelling?t('studio.cancelling', { defaultValue: "Cancelling…" }):t('studio.cancelTask', { defaultValue: "Cancel task" })}</Button></Flex></section>}
    {!a?.activity.busy&&['waiting-for-game','waiting-for-compute'].includes(a?.activity.phase??'')&&<section className="studio-card"><h3>{t('studio.requestWaiting', { defaultValue: "Your request is waiting" })}</h3><p>{a?.activity.progress}</p><p className="studio-caption">{t('studio.retryWhenAvailable', { defaultValue: "Retry when the game or processing slot is available." })}</p>{['sync','install-models'].includes(a?.activity.task??'')&&<Button variant="soft" disabled={a?.gamePresence==='external'||a?.gamePresence==='unknown'} onClick={()=>void state.run(()=>automationApi.action({command:a!.activity.task}))}>{t('studio.retryTask', { defaultValue: "Retry task" })}</Button>}</section>}
    {list.length===0&&<div className="studio-empty-inline"><Icon name="activity" size={30}/><h3>{t('studio.noTasks', { defaultValue: "No recording tasks here" })}</h3><p>{t('studio.exportFromMatchesDescription', { defaultValue: "Export a moment from Matches to add it to the queue." })}</p><Button variant="soft" onClick={()=>onNavigate({page:'matches'})}>{t('studio.browseMatches', { defaultValue: "Browse matches" })}</Button></div>}
    <div className="activity-list">{pageJobs.map(job=>{
      const demo=demosById.get(job.demoId);
      const busy=['running','queued'].includes(job.status);
      const isCancelling=cancelling.has(job.id)||job.stage==='cancelling';
      return <section className="studio-card" key={job.id} data-job-id={job.id}><Flex justify="between" align="start" gap="3" wrap="wrap"><div>{isReviewPreview(job)&&<Badge color="gray">{t('studio.reviewPreview',{defaultValue:'Review preview'})}</Badge>}<h3>{demo?.mapName||job.sourceSnapshot?.demoInfo.mapName||demo?.name||t('studio.matchExport', { defaultValue: "Match export" })}</h3><p>{t('studio.taskRequestedFormat',{defaultValue:'{{count}} moments · Requested {{height}}p / {{fps}} FPS',count:job.highlightIds.length,height:job.options.height,fps:job.options.fps})}</p></div><Badge color={job.status==='done'?'green':job.status==='error'?'red':'amber'}>{isCancelling?t('studio.cancelling', { defaultValue: "Cancelling…" }):t(`renders.status.${job.status}`)}</Badge></Flex>
      {job.stage&&<p>{job.stage}</p>}{job.progress!=null&&job.status==='running'&&<Progress value={Math.min(100,job.progress*100)} aria-label={t('studio.exportProgress', { defaultValue: "Export progress" })}/>}{job.error&&<p className="studio-error">{job.error}</p>}
      {job.outputs.length>0&&<p>{t('studio.savedFilesAvailable',{defaultValue:'{{count}} saved file(s) available',count:job.outputs.length})}</p>}
      <Flex gap="2" wrap="wrap" mt="3"><Button variant="soft" disabled={source.pending.has(job.id)} onClick={()=>void source.open(job)}>{source.pending.has(job.id)?t('studio.openingSource', { defaultValue: "Opening source…" }):t('studio.openMatchExports', { defaultValue: "Open match exports" })}</Button>
      {busy&&<Button variant="outline" color="red" disabled={isCancelling} onClick={()=>void cancel(job)}>{t('studio.cancelExport', { defaultValue: "Cancel export" })}</Button>}
      {['error','partial','cancelled'].includes(job.status)&&(job.sourceSnapshot||job.analysisClips)&&<Button variant="outline" disabled={actions.pending.has('retry:'+job.id)} onClick={()=>void actions.run('retry:'+job.id,()=>state.run(()=>api.retryRender(job.id)))}>{actions.pending.has('retry:'+job.id)?t('studio.queuingRetry', { defaultValue: "Queuing retry…" }):t('studio.retryOriginalSettings', { defaultValue: "Retry with original settings" })}</Button>}
      {['error','partial','cancelled'].includes(job.status)&&!job.sourceSnapshot&&!job.analysisClips&&<Button variant="outline" disabled={source.pending.has(job.id)} onClick={()=>void source.open(job,'moments')}>{t('studio.openMatchExportAgain', { defaultValue: "Open moments to export again" })}</Button>}
      {job.log.length>0&&<TaskLog lines={job.log}/>} </Flex></section>;
    })}</div>
    {pages>1&&<Flex align="center" justify="between" gap="3" wrap="wrap" aria-label={t('studio.historyPages', { defaultValue: "Recording history pages" })}><span>{t('studio.historyRange',{defaultValue:'{{first}}–{{last}} of {{count}} tasks',first:currentPage*PAGE_SIZE+1,last:Math.min((currentPage+1)*PAGE_SIZE,list.length),count:list.length})}</span><Flex gap="2"><Button variant="soft" disabled={currentPage===0} onClick={()=>setPage(currentPage-1)}>{t('studio.previousPage', { defaultValue: "Previous page" })}</Button><Button variant="soft" disabled={currentPage===pages-1} onClick={()=>setPage(currentPage+1)}>{t('studio.nextPage', { defaultValue: "Next page" })}</Button></Flex></Flex>}
    {state.demos.some(d=>['queued','validating','parsing','error'].includes(d.status))&&<section className="studio-card"><h2>{t('studio.matchPreparation', { defaultValue: "Match preparation" })}</h2>{state.demos.filter(d=>d.status!=='parsed').map(d=><div className="studio-list-row" key={d.id}><span>{d.mapName||d.name}</span><Badge color={d.status==='error'?'red':'amber'}>{t(`demoList.status.${d.status}`)}</Badge><Button variant="ghost" onClick={()=>onNavigate({page:'matches',demoId:d.id})}>{t('studio.openMatch', { defaultValue: "Open match" })}</Button></div>)}</section>}
    {state.analysisJobs.length>0&&<section className="studio-card"><h2>{t('studio.gameplayAnalysis', { defaultValue: "Gameplay analysis" })}</h2>{state.analysisJobs.slice(0,30).map(job=><div className="studio-list-row" key={job.id}><span>{state.demos.find(d=>d.id===job.demoId)?.mapName||t('studio.yourMatch',{defaultValue:'Your match'})}{job.phase==='waiting-for-compute'?' · '+t('studio.waitingProcessingSlot',{defaultValue:'Waiting for the processing slot'}):job.step?' · '+t('studio.analysisStep',{defaultValue:'Step {{step}} of 3',step:job.step}):''}</span><Badge>{t(`renders.status.${job.status}`,{defaultValue:job.status})}</Badge><Button variant="ghost" onClick={()=>onNavigate({page:'matches',demoId:job.demoId,tab:'scoring'})}>{t('studio.openAnalysis', { defaultValue: "Open analysis" })}</Button>{job.status==='error'&&<Button variant="soft" onClick={()=>void state.run(()=>api.scoreMatch(job.demoId))}>{t('studio.retryAnalysis', { defaultValue: "Retry analysis" })}</Button>}</div>)}</section>}
    <section className="studio-card"><h2>{t('studio.downloadsInstallation', { defaultValue: "Downloads and installation" })}</h2>{Object.entries(state.setup).filter(([,value])=>value?.running).map(([tool,value])=><div className="studio-list-row" key={tool}><span>{tool+' · '+(value?.progress||t('studio.working', { defaultValue: "Working…" }))}</span><Button variant="soft" color="red" onClick={()=>void state.run(()=>api.cancelSetup(tool as 'hlae'|'ffmpeg'|'vrf'))}>{t('studio.cancelDownload', { defaultValue: "Cancel download" })}</Button></div>)}
    {a?.matches?.map((match,index)=><div className="studio-list-row" key={match.code}><span>{t('studio.steamMatchNumber',{defaultValue:'Steam match {{number}}',number:index+1})}{match.attempts?t('studio.downloadAttempt',{defaultValue:' · attempt {{count}}',count:match.attempts}):''}{match.error&&<small className="studio-error">{match.error}</small>}</span><Badge>{match.status}</Badge>{match.nextRetry&&<small>{t('studio.downloadRetryAfter',{defaultValue:'Retry after {{time}}',time:new Date(match.nextRetry*1000).toLocaleString(i18n.language)})}</small>}{!match.path&&<Button variant="soft" disabled={a.activity.busy} onClick={()=>void state.run(()=>automationApi.action({command:'retry',code:match.code}))}>{t('studio.retryDownload', { defaultValue: "Retry download" })}</Button>}</div>)}</section>
  </div>;
}
