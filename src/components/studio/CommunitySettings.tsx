import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { Badge, Button, Checkbox, Dialog, Flex, Progress, Select, TextArea, TextField } from '@radix-ui/themes';
import { useTranslation } from 'react-i18next';
import { communityApi, type CommunityStatus, type Contribution, type ReviewerExample } from '../../api_community';
import { errorText, mb } from '../../api';
import { api } from '../../api';
import { isReviewPreview } from '../../renderPurpose';
import { useStudio } from '../../StudioProvider';
import type { DraftGuard } from '../../studioState';
import { ClipPlayer } from './ClipPlayer';
import { learningCategoryLabel } from './FeedbackDialog';

const EMPTY_SETTINGS = {sharingEnabled:false,speechEnabled:false};
const ACTIVE_STATUSES = ['preparing','queued','uploading','cancelling','withdrawing'];
function RegisteredPreview({id,action,title,disabled=false}:{id:string;action:'contribution_preview'|'reviewer_open';title:string;disabled?:boolean}) {
  const [open,setOpen]=useState(false);
  const [media,setMedia]=useState<string>();
  const [busy,setBusy]=useState(false);
  const [error,setError]=useState<string>();
  const generation=useRef(0);
  const preview=async()=>{
    if(busy)return;
    const revision=++generation.current;
    setBusy(true);setError(undefined);setOpen(true);setMedia(undefined);
    try{const result=await communityApi.action<{mediaPath:string}>(action,{id});if(revision===generation.current){if(!result.mediaPath)throw new Error('No reviewable media is available.');setMedia(result.mediaPath);}}catch(e){if(revision===generation.current)setError(errorText(e));}finally{if(revision===generation.current)setBusy(false);}
  };
  useEffect(()=>()=>{generation.current++;},[]);
  return <><Button variant="soft" disabled={disabled||busy} onClick={()=>void preview()}>Inspect clip</Button><Dialog.Root open={open} onOpenChange={value=>{if(!value){generation.current++;setBusy(false);setMedia(undefined);}setOpen(value);}}><Dialog.Content maxWidth="900px"><Dialog.Title>{title}</Dialog.Title><Dialog.Description>Registered media verified by ClipForge before playback.</Dialog.Description>{busy&&<p role="status">Preparing verified preview…</p>}{media&&<div className="clip-detail-stage"><ClipPlayer src={api.fileSrc(media)} title={title}/></div>}{error&&<p role="alert" className="studio-error">{error}</p>}<Flex justify="end" mt="3"><Dialog.Close><Button variant="soft">Close preview</Button></Dialog.Close></Flex></Dialog.Content></Dialog.Root></>;
}
export function CommunityTasks({onOpen}: {onOpen?:()=>void}) {
  const studio=useStudio();
  const [pending,setPending]=useState(new Set<string>());
  const [error,setError]=useState<string>();
  const act=async(action:string,id:string)=>{
    if(pending.has(id))return;
    setPending(current=>new Set(current).add(id));setError(undefined);
    try{await communityApi.action(action,{id});await studio.refreshCommunity();}catch(e){setError(errorText(e));}finally{setPending(current=>{const next=new Set(current);next.delete(id);return next;});}
  };
  const contributions=studio.community?.contributions??[];
  if(!contributions.length&&!studio.community?.activity?.busy&&!studio.community?.activity?.error)return null;
  return <section className="studio-card community-contributions"><Flex align="center" justify="between" gap="3" wrap="wrap"><h2>Community contributions</h2>{onOpen&&<Button variant="soft" onClick={onOpen}>Open Community & learning</Button>}</Flex>
    {studio.community?.activity?.busy&&<Flex gap="3" align="center" justify="between" wrap="wrap"><p role="status">{studio.community.activity.cancelling?'Cancelling…':studio.community.activity.progress||studio.community.activity.phase||'Working…'}</p><Button variant="outline" color="red" disabled={pending.has('')||studio.community.activity.cancelling} onClick={()=>void act('cancel','')}>{studio.community.activity.cancelling?'Cancelling…':'Cancel community task'}</Button></Flex>}
    {studio.community?.activity?.error&&<p role="alert" className="studio-error">{studio.community.activity.error}</p>}
    {contributions.map(item=><ContributionCard key={item.id} item={item} pending={pending.has(item.id)} act={act} sharingEnabled={!!studio.community?.settings.sharingEnabled}/>)}
    {error&&<p role="alert" className="studio-error">{error}</p>}
  </section>;
}
function ContributionCard({item,pending,act,sharingEnabled}:{item:Contribution;pending:boolean;act:(action:string,id:string)=>Promise<void>;sharingEnabled:boolean}) {
  const [confirmed,setConfirmed]=useState(false);
  const [withdrawConfirm,setWithdrawConfirm]=useState(false);
  const files=item.preview?.files??[];
  const total=item.totalBytes>0?item.totalBytes:undefined;
  return <article className="community-contribution" aria-label={`Contribution ${item.id}`}><Flex align="center" justify="between" gap="2" wrap="wrap"><h3>{item.kind==='demo'?'Full demo contribution':'Selected clip example'}</h3><Badge color={['error','needs_attention'].includes(item.status)?'red':['uploaded','submitted'].includes(item.status)?'green':'gray'}>{item.status}</Badge></Flex>
    <p>{item.preview?.description}</p><details open={['prepared','ready'].includes(item.status)}><summary>Exactly what this package contains</summary><ul>{files.map(file=><li key={file.name}>{file.name} · {mb(file.sizeBytes)}</li>)}</ul><p>{item.preview?.containsVoice?'Voice/audio may be present.':'No recorded voice reported in this package.'} {item.preview?.containsChat?'Player names and chat may be present.':'No chat reported in this package.'}</p>{item.kind==='demo'&&<p>A raw demo can contain identities, server details, chat and recorded voice. Turning speech learning off does not remove these from the raw file.</p>}<p>{(item.preview?.categories??[]).join(' · ')}</p></details>
    {total&&<><p>{mb(Math.min(item.sentBytes,total))} of {mb(total)} transferred</p>{item.status==='uploading'&&<Progress value={Math.min(100,100*item.sentBytes/total)} aria-label="Contribution upload progress"/>}</>}
    {item.error&&<p role="alert" className="studio-error">{item.error}</p>}{item.receipt&&<p className="community-receipt">Contribution receipt: {typeof item.receipt==='string'?item.receipt:item.receipt.id}{typeof item.receipt==='object'&&item.receipt.state?' · '+item.receipt.state:''}</p>}
    {item.status==='withdrawal_pending'&&<p role="status">Withdrawal is waiting for the receiver. This package is blocked from upload and offline export until withdrawal is resolved.</p>}
    {['prepared','ready','paused','error','retry','needs_attention'].includes(item.status)&&<label className="community-check"><Checkbox checked={confirmed} onCheckedChange={value=>setConfirmed(value===true)}/><span>I reviewed this package and want to share it for AI training.</span></label>}
    <Flex gap="2" wrap="wrap">
      {['prepared','ready'].includes(item.status)&&<Button disabled={pending||!sharingEnabled||!confirmed} onClick={()=>void act('contribution_upload',item.id)}>Share this package</Button>}
      {item.kind==='example'&&['prepared','ready','paused','submitted','needs_attention'].includes(item.status)&&<RegisteredPreview id={item.id} action="contribution_preview" title="Prepared contribution preview" disabled={pending}/>}
      {['prepared','ready','paused','uploaded','error','submitted','needs_attention'].includes(item.status)&&<Button variant="outline" disabled={pending} onClick={()=>void act('contribution_export',item.id)}>Save offline package</Button>}
      {item.status==='uploading'&&<Button variant="soft" disabled={pending} onClick={()=>void act('contribution_pause',item.id)}>Pause upload</Button>}
      {['paused','error','retry','needs_attention'].includes(item.status)&&<Button disabled={pending||!sharingEnabled||!confirmed} onClick={()=>void act('contribution_retry',item.id)}>Retry / resume upload</Button>}
      {ACTIVE_STATUSES.includes(item.status)&&<Button variant="outline" color="red" disabled={pending||item.status==='cancelling'||item.status==='withdrawing'} onClick={()=>void act('contribution_cancel',item.id)}>{item.status==='cancelling'?'Cancelling…':'Cancel task'}</Button>}
      {(item.receipt||item.remoteId)&&!['withdrawn','withdrawing','withdrawal_pending'].includes(item.status)&&<Button variant="outline" color="red" disabled={pending} onClick={()=>setWithdrawConfirm(true)}>Withdraw contribution</Button>}
      {item.status==='withdrawal_pending'&&<Button variant="outline" color="red" disabled={pending} onClick={()=>void act('contribution_withdraw',item.id)}>Retry withdrawal</Button>}
    </Flex>{withdrawConfirm&&<div className="community-confirm" role="group" aria-label="Confirm contribution withdrawal"><p>Delete the server contribution and exclude it from future training. Previously released model weights may retain its influence.</p><Flex gap="2" wrap="wrap"><Button variant="soft" onClick={()=>setWithdrawConfirm(false)}>Keep contribution</Button><Button color="red" disabled={pending} onClick={()=>{setWithdrawConfirm(false);void act('contribution_withdraw',item.id);}}>Confirm withdrawal</Button></Flex></div>}
  </article>;
}

export function CommunitySettings({onGuardChange}:{onGuardChange:(guard:DraftGuard|undefined)=>void}) {
  const {t}=useTranslation();
  const studio=useStudio();
  const status=studio.community;
  const id=useId();
  const [draft,setDraft]=useState(EMPTY_SETTINGS);
  const [token,setToken]=useState('');
  const [saving,setSaving]=useState(false);
  const [busy,setBusy]=useState(false);
  const [error,setError]=useState<string>();
  const [message,setMessage]=useState<string>();
  const [kind,setKind]=useState<'demo'|'example'>('example');
  const [source,setSource]=useState('');
  const [notes,setNotes]=useState('');
  const [rights,setRights]=useState(false);
  const [personal,setPersonal]=useState(false);
  const [trainingConsent,setTrainingConsent]=useState(false);
  const [includeAudio,setIncludeAudio]=useState(false);
  const [includeFeedback,setIncludeFeedback]=useState(false);
  const [reviewerItems,setReviewerItems]=useState<ReviewerExample[]>([]);
  const canReview=!!status?.operator?.configured&&['reviewer','trainer','operator'].includes(status.operator.role);
  const canTrain=!!status?.operator?.configured&&['trainer','operator'].includes(status.operator.role);
  const taskBusy=busy||!!status?.activity?.busy||status?.training?.state==='busy';
  // Native status returns release history in newest-first order.
  const latestCandidate=status?.training?.versions.find(version=>!version.active&&['trained','needs_evaluation','evaluated'].includes(version.status??''));
  const activeVersion=status?.training?.versions.find(version=>version.active);
  const loaded=useRef(false);
  const dirty=draft.sharingEnabled!==(status?.settings.sharingEnabled??false)||draft.speechEnabled!==(status?.settings.speechEnabled??false)||token.length>0;
  useEffect(()=>{if(status&&!loaded.current){loaded.current=true;setDraft({sharingEnabled:status.settings.sharingEnabled,speechEnabled:status.settings.speechEnabled});}},[status]);
  const discard=useCallback(()=>{setDraft(status?{sharingEnabled:status.settings.sharingEnabled,speechEnabled:status.settings.speechEnabled}:EMPTY_SETTINGS);setToken('');setError(undefined);},[status?.settings.sharingEnabled,status?.settings.speechEnabled]);
  const save=useCallback(async()=>{
    if(saving||!status)return false;
    setSaving(true);setError(undefined);
    try {await communityApi.action('preferences_save',draft);if(token.trim()){await communityApi.action('operator_configure',{token:token.trim()});setToken('');}await studio.refreshCommunity();setMessage('Community preferences saved.');return true;}catch(e){setError(errorText(e));return false;}finally{setSaving(false);}
  },[saving,status,draft,token,studio.refreshCommunity]);
  useEffect(()=>{onGuardChange({dirty,save,discard});return()=>onGuardChange(undefined);},[dirty,save,discard,onGuardChange]);
  const run=async(action:string,values:Record<string,unknown>={})=>{
    if(busy)return;
    setBusy(true);setError(undefined);setMessage(undefined);
    try {const result=await communityApi.action<CommunityStatus&{items?:ReviewerExample[]}>(action,values);if(action==='reviewer_list')setReviewerItems(result.items??[]);await studio.refreshCommunity();return true;}catch(e){setError(errorText(e));return false;}finally{setBusy(false);}
  };
  const sources=kind==='demo'?studio.demos.map(demo=>({key:demo.id,label:[demo.mapName,demo.name].filter(Boolean).join(' · ')})):studio.jobs.filter(job=>!isReviewPreview(job)).flatMap(job=>job.outputs.map(output=>({key:JSON.stringify([job.id,output.file]),label:output.title})));
  const chooseSource=(value:string)=>{setSource(value);setRights(false);setPersonal(false);setTrainingConsent(false);setIncludeAudio(false);setIncludeFeedback(false);};
  useEffect(()=>{if(source&&!sources.some(item=>item.key===source))setSource('');},[kind,studio.demos,studio.jobs,source]);
  const prepare=async()=>{
    if(!source||!rights||!personal||!trainingConsent)return;
    const selected=kind==='demo'?{demoId:source}:(()=>{const [jobId,outputKey]=JSON.parse(source) as [string,string];return {jobId,outputKey};})();
    const feedbackIds=includeFeedback?(status?.feedback??[]).filter(row=>kind==='demo'?row.demoId===source:row.jobId===('jobId' in selected?selected.jobId:'')&&row.outputKey===('outputKey' in selected?selected.outputKey:'')).slice(0,64).map(row=>row.id):[];
    const ok=await run('contribution_prepare',{kind,...selected,feedbackIds,consent:{training:true,rightsConfirmed:true,personalDataAcknowledged:true,includeAudio:includeAudio&&!!status?.settings.speechEnabled},notes:notes.trim()});
    if(ok){setMessage('Preparation requested. Review the exact package below before sharing.');setRights(false);setPersonal(false);setTrainingConsent(false);setIncludeAudio(false);setIncludeFeedback(false);}
  };
  return <div className="community-settings">
    <section className="studio-card"><span className="studio-eyebrow">LEARN TOGETHER</span><h2>{t('studio.communityLearning',{defaultValue:'Community & learning'})}</h2><p>Help ClipForge find the clips you would otherwise miss. Feedback is local; sharing footage is optional and reviewed before transfer.</p>
      {studio.communityError&&<p role="alert" className="studio-error">Community tools are unavailable: {studio.communityError}</p>}
      <p><strong>{status?.feedback.length??0}</strong> saved feedback examples · <strong>{status?.training?.verifiedExamples??0}</strong> verified training examples</p>
      <p className="studio-caption">Saved preferences and approved examples can inform future reviews. Reviewing matches does not automatically retrain the model.</p>
      <form className="community-form" onSubmit={event=>{event.preventDefault();void save();}}>
        <label className="community-check"><Checkbox disabled={!status||saving} checked={draft.sharingEnabled} onCheckedChange={value=>setDraft(current=>({...current,sharingEnabled:value===true}))}/><span>Allow me to share reviewed packages<small>Nothing uploads automatically. Each package needs your confirmation.</small></span></label>
        <label className="community-check"><Checkbox disabled={!status||saving} checked={draft.speechEnabled} onCheckedChange={value=>setDraft(current=>({...current,speechEnabled:value===true}))}/><span>Allow optional speech examples for training<small>Separate consent is required for each example containing audio.</small></span></label>
        <p className="community-endpoint">Receiver: {status?.settings.endpoint??'https://157.173.114.38:9443'}</p>
        <Flex gap="3" wrap="wrap"><Button disabled={!status||!dirty||saving} type="submit">{saving?'Saving…':'Save community preferences'}</Button><Button variant="soft" disabled={!dirty||saving} type="button" onClick={discard}>Discard edits</Button></Flex>
      </form>
    </section>
    <section className="studio-card"><h2>Contribute a demo or missed moment</h2><p>Choose a registered match or clip. Steam credentials are never included. Raw demos are retained for 30 days; approved minimized examples remain until withdrawal.</p><div className="community-form">
      <label htmlFor={id+'-kind'}>Contribution type</label><Select.Root value={kind} onValueChange={value=>{setKind(value as 'demo'|'example');chooseSource('');}}><Select.Trigger id={id+'-kind'}/><Select.Content><Select.Item value="example">Selected clip example</Select.Item><Select.Item value="demo">Full match demo</Select.Item></Select.Content></Select.Root>
      <label htmlFor={id+'-source'}>{kind==='demo'?'Registered match':'Registered clip'}</label><Select.Root value={source} onValueChange={chooseSource}><Select.Trigger id={id+'-source'} placeholder={kind==='demo'?'Choose a match':'Choose a clip'}/><Select.Content>{sources.map(item=><Select.Item key={item.key} value={item.key}>{item.label}</Select.Item>)}</Select.Content></Select.Root>
      {sources.length===0&&<p>No {kind==='demo'?'matches':'clips'} are available yet. Import a demo or export a moment first.</p>}
      {kind==='demo'&&<div className="community-notice"><strong>A full demo is not anonymous.</strong><p>It may include all players’ identities, server information, chat and recorded voice. Speech processing consent does not strip voice from a raw demo.</p></div>}
      <label htmlFor={id+'-notes'}>What can this teach the app?</label><TextArea id={id+'-notes'} maxLength={800} value={notes} onChange={event=>setNotes(event.currentTarget.value)} placeholder="Tell us about a whiff, an unusual shot, a missed blooper, or ordinary moments the app should ignore."/>
      <label className="community-check"><Checkbox checked={includeFeedback} onCheckedChange={value=>setIncludeFeedback(value===true)}/><span>Include my related saved feedback and corrections<small>Only feedback for this selected source is added. Review its notes below before preparing.</small></span></label>
      {includeFeedback&&<ul>{(status?.feedback??[]).filter(row=>kind==='demo'?row.demoId===source:source===JSON.stringify([row.jobId,row.outputKey])).slice(0,64).map(row=><li key={row.id}>{learningCategoryLabel(row.category??'other')} · {row.rating}{row.notes?' · '+row.notes:''}</li>)}</ul>}
      <label className="community-check"><Checkbox checked={trainingConsent} onCheckedChange={value=>setTrainingConsent(value===true)}/><span>I consent to this contribution being used for AI training.</span></label>
      <label className="community-check"><Checkbox checked={rights} onCheckedChange={value=>setRights(value===true)}/><span>I have the right to contribute this gameplay and understand what I am sharing.</span></label>
      <label className="community-check"><Checkbox checked={personal} onCheckedChange={value=>setPersonal(value===true)}/><span>I understand that player names, chat, voice or other personal information may be present.</span></label>
      <label className="community-check"><Checkbox disabled={!status?.settings.speechEnabled} checked={includeAudio} onCheckedChange={value=>setIncludeAudio(value===true)}/><span>I separately consent to audio/speech processing for this example.</span></label>
      <Button disabled={!status||busy||!source||!rights||!personal||!trainingConsent} onClick={()=>void prepare()}>{busy?'Preparing…':'Prepare package for review'}</Button>
    </div></section>
    <CommunityTasks/>
    <section className="studio-card"><h2>Model learning status</h2><Badge>{status?.training?.state??'Checking status'}</Badge><p>{status?.training?.message??'No trained update has been verified.'}</p>{status?.training&&<p>Student: {status.training.model}<br/>Teacher candidate: {status.training.teacher}<br/>{status.training.examples} examples · {status.training.verifiedExamples} verified</p>}<p className="studio-caption">Teacher judgments are provisional. Updates must pass tests on unseen matches before promotion. The previous model remains available for rollback.</p>
      {(status?.training?.versions??[]).map(version=><div className="community-model-version" key={version.id}><strong>{version.id}</strong><Badge color={version.active?'green':'gray'}>{version.active?'Active':version.status??'Unverified'}</Badge><p>{version.kind==='preference-logistic-v1'?'CPU preference learner — Qwen vision weights unchanged':version.kind==='qwen3-vl-lora-v1'?'Qwen LoRA adapter — compatibility and quality gates required':'Model type not reported'}</p><p>{version.message}</p>{canTrain&&version.passed===true&&!version.active&&<Button disabled={taskBusy} onClick={()=>void run('promote',{versionId:version.id})}>Promote tested version</Button>}</div>)}
    </section>
    <section className="studio-card"><h2>Private reviewer tools</h2><p>For project reviewers only. Access uses a separate revocable credential; it does not use your Steam authentication.</p><label htmlFor={id+'-operator'}>Reviewer credential</label><TextField.Root id={id+'-operator'} type="password" autoComplete="off" value={token} maxLength={512} onChange={event=>setToken(event.currentTarget.value)} placeholder="Enter a reviewer credential"/><p className="studio-caption">The draft stays in memory and clears after a successful save. Native storage protects the saved credential.</p><Button disabled={!status||!token.trim()||saving} onClick={()=>void save()}>Save reviewer credential</Button>
      {canReview&&<><p>Role: {status!.operator!.role}</p><Flex gap="2" wrap="wrap"><Button disabled={taskBusy} variant="soft" onClick={()=>void run('reviewer_list')}>Load private examples</Button></Flex>
        {canTrain&&<><h3>Training and release tools</h3><p>CPU learning fits clip preferences without changing Qwen’s vision weights. The Qwen LoRA pilot requires the separate hardware, checkpoint, inference and held-out quality checks.</p><Flex gap="2" wrap="wrap"><Button disabled={taskBusy} variant="soft" onClick={()=>void run('teacher_review')}>Run teacher review</Button><Button disabled={taskBusy||!status?.training?.verifiedExamples} variant="soft" onClick={()=>void run('train',{kind:'calibration'})}>Fit CPU preference learner</Button><Button disabled={taskBusy||!status?.training?.verifiedExamples} variant="soft" onClick={()=>void run('train',{kind:'lora'})}>Train guarded Qwen LoRA pilot</Button><Button disabled={taskBusy||!latestCandidate} variant="soft" onClick={()=>latestCandidate&&void run('evaluate',{versionId:latestCandidate.id})}>Evaluate latest candidate</Button><Button disabled={taskBusy||!activeVersion} variant="outline" onClick={()=>void run('rollback')}>Restore previous model</Button></Flex><p className="studio-caption">{latestCandidate?'Evaluation target: '+latestCandidate.id:'No eligible trained checkpoint is available for evaluation.'}{activeVersion?' · Active release: '+activeVersion.id:' · Built-in model active; no trained release to roll back.'}</p></>}
        {reviewerItems.map(item=><article key={item.id} className="community-contribution"><h3>{item.title??item.id}</h3><p>{item.category} · {item.status}</p><p>{item.notes}</p>{item.teacherLabel&&<p>Provisional teacher label: {item.teacherLabel}</p>}{item.groundTruth&&<p>Demo facts: {item.groundTruth}</p>}<Flex gap="2" wrap="wrap"><RegisteredPreview id={item.id} action="reviewer_open" title={item.title??'Private example'} disabled={!item.mediaAvailable||busy}/><Button disabled={busy||!item.mediaAvailable} onClick={()=>void run('reviewer_decide',{id:item.id,decision:'approved'})}>Approve example</Button><Button disabled={busy} color="red" variant="soft" onClick={()=>void run('reviewer_decide',{id:item.id,decision:'rejected'})}>Reject example</Button></Flex></article>)}
      </>}
    </section>
    {message&&<p role="status">{message}</p>}{error&&<p role="alert" className="studio-error">{error}</p>}
  </div>;
}
