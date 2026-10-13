import { useEffect, useId, useState } from 'react';
import { Badge, Button, Dialog, Flex, Select, Text, TextArea, TextField } from '@radix-ui/themes';
import { useTranslation } from 'react-i18next';
import { communityApi, type FeedbackRating, type FeedbackSource } from '../../api_community';
import { errorText } from '../../api';
import { useOptionalStudio } from '../../StudioProvider';

export const LEARNING_CATEGORIES = ['highlight', 'exceptional_shot', 'whiff', 'movement_mistake', 'surprise', 'friendly_fire', 'knife', 'timing', 'chaos', 'banter', 'reaction', 'death', 'tension', 'utility', 'other'] as const;
const LABELS: Record<string, string> = {highlight:'Highlight',exceptional_shot:'Exceptional shot',whiff:'Extreme whiff',movement_mistake:'Movement mistake',surprise:'Unexpected surprise',friendly_fire:'Friendly fire',knife:'Knife / taser moment',timing:'Bad timing',chaos:'Funny blooper / chaos',banter:'Funny banter',reaction:'Reaction',death:'Death',utility:'Utility mistake',tension:'Tense moment',other:'Other / ordinary'};
export function learningCategoryLabel(value: string) {return LABELS[value] ?? value;}
interface Draft {rating: FeedbackRating; category: string; notes: string; start: string; end: string}
const drafts = new Map<string, Draft>();
const sourceKey = (source: FeedbackSource) => [source.sourceKind,source.demoId,source.jobId,source.outputKey,source.momentId,source.playerId,source.sourceKind === 'match' ? source.startTick : ''].join('|');

/** Closing retains an unsaved local draft; feedback never uploads footage. */
export function FeedbackButton({source,tickRate,disabled=false,label,defaultRating='good',onOpen}: {source: FeedbackSource;tickRate?:number;disabled?:boolean;label?:string;defaultRating?:FeedbackRating;onOpen?:()=>void}) {
  const {t}=useTranslation();
  const [open,setOpen]=useState(false);
  const [snapshot,setSnapshot]=useState(source);
  return <><Button variant="soft" disabled={disabled} onClick={event=>{event.stopPropagation();onOpen?.();setSnapshot(source);setOpen(true);}}>{label??t('studio.makeAppBetter',{defaultValue:'Make the app better'})}</Button><FeedbackDialog open={open} onOpenChange={setOpen} source={snapshot} tickRate={tickRate} defaultRating={defaultRating}/></>;
}
export function FeedbackDialog({source,open,onOpenChange,tickRate,defaultRating='good'}:{source:FeedbackSource;open:boolean;onOpenChange:(open:boolean)=>void;tickRate?:number;defaultRating?:FeedbackRating}) {
  const {t}=useTranslation();
  const studio=useOptionalStudio();
  const id=useId();
  const key=sourceKey(source);
  const initial=():Draft=>drafts.get(key)??{rating:defaultRating,category:'other',notes:'',start:source.startTick!==undefined&&tickRate?String((source.startTick/tickRate).toFixed(2)):'',end:source.endTick!==undefined&&tickRate?String((source.endTick/tickRate).toFixed(2)):''};
  const [draft,setDraft]=useState<Draft>(initial);
  const [busy,setBusy]=useState(false);
  const [error,setError]=useState<string>();
  const [saved,setSaved]=useState(false);
  useEffect(()=>{if(open){setDraft(initial());setError(undefined);setSaved(false);}},[open,key]);
  const update=(values:Partial<Draft>)=>setDraft(current=>{const next={...current,...values};drafts.set(key,next);return next;});
  const hasTiming=!!tickRate&&source.startTick!==undefined&&source.endTick!==undefined;
  const start=Number(draft.start),end=Number(draft.end);
  const validTiming=!hasTiming||(draft.start.trim()!==''&&draft.end.trim()!==''&&Number.isFinite(start)&&Number.isFinite(end)&&start>=0&&end>start);
  const save=async()=>{
    if(busy||!validTiming)return;
    setBusy(true);setError(undefined);
    try {
      await communityApi.action('feedback_save',{...source,rating:draft.rating,category:draft.category,notes:draft.notes.trim(),...(hasTiming?{startTick:Math.round(start*tickRate!),endTick:Math.round(end*tickRate!)}:{})});
      drafts.delete(key);setSaved(true);await studio?.refreshCommunity();
    } catch(e){setError(errorText(e));}finally{setBusy(false);}
  };
  return <Dialog.Root open={open} onOpenChange={value=>{if(!busy)onOpenChange(value);}}><Dialog.Content maxWidth="640px" className="community-dialog"><Dialog.Title>{t('studio.makeAppBetter',{defaultValue:'Make the app better'})}</Dialog.Title><Dialog.Description>{t('studio.feedbackLocalDescription',{defaultValue:'Teach ClipForge what is worth keeping. Feedback saves on your PC; sharing is a separate choice.'})}</Dialog.Description>
    {saved?<div role="status" className="community-feedback-saved"><Badge color="green">{t('studio.feedbackSaved',{defaultValue:'Feedback saved locally'})}</Badge><p>{t('studio.feedbackSavedHint',{defaultValue:'Your original suggestion and correction are kept together. This does not mean a new model has been trained.'})}</p><Button onClick={()=>onOpenChange(false)}>{t('common.close')}</Button></div>:<form className="community-form" onSubmit={event=>{event.preventDefault();void save();}}>
      {source.originalProposal&&<details><summary>{t('studio.originalSuggestion',{defaultValue:'Original suggestion'})}</summary><p>{source.originalProposal.title}</p><Flex gap="1" wrap="wrap">{source.originalProposal.tags.map(tag=><Badge key={tag} color="gray">{tag}</Badge>)}</Flex>{source.originalProposal.confidence!==undefined&&<p>Model score: {source.originalProposal.confidence}/100</p>}</details>}
      <label htmlFor={id+'-rating'}>{t('studio.feedbackVerdict',{defaultValue:'Your verdict'})}</label><Select.Root value={draft.rating} onValueChange={rating=>update({rating:rating as FeedbackRating})}><Select.Trigger id={id+'-rating'}/><Select.Content><Select.Item value="good">Worth keeping</Select.Item><Select.Item value="wrong">Incorrect or boring</Select.Item><Select.Item value="missed">The app missed this</Select.Item><Select.Item value="timing">Correct the timing</Select.Item></Select.Content></Select.Root>
      <label htmlFor={id+'-category'}>{t('studio.correctCategory',{defaultValue:'Correct category'})}</label><Select.Root value={draft.category} onValueChange={category=>update({category})}><Select.Trigger id={id+'-category'}/><Select.Content>{LEARNING_CATEGORIES.map(category=><Select.Item key={category} value={category}>{learningCategoryLabel(category)}</Select.Item>)}</Select.Content></Select.Root>
      {hasTiming&&<div className="community-two-fields"><label>{t('studio.startMatchSeconds',{defaultValue:'Start (match seconds)'})}<TextField.Root type="number" min="0" step="0.01" value={draft.start} onChange={event=>update({start:event.currentTarget.value})}/></label><label>{t('studio.endMatchSeconds',{defaultValue:'End (match seconds)'})}<TextField.Root type="number" min="0" step="0.01" value={draft.end} onChange={event=>update({end:event.currentTarget.value})}/></label></div>}
      {!validTiming&&<Text color="red" role="alert">End must come after start, and both times must be valid.</Text>}
      <label htmlFor={id+'-notes'}>{t('studio.explainMoment',{defaultValue:'What happened? What should the app notice?'})}</label><TextArea id={id+'-notes'} maxLength={800} value={draft.notes} placeholder="For example: a close-range spray missed, then the player fell off the ledge." onChange={event=>update({notes:event.currentTarget.value})}/>
      {error&&<p className="studio-error" role="alert">{error}</p>}<Flex justify="end" gap="3" wrap="wrap"><Button type="button" variant="soft" disabled={busy} onClick={()=>onOpenChange(false)}>{t('studio.keepFeedbackDraft',{defaultValue:'Close, keep draft'})}</Button><Button type="submit" disabled={busy||!validTiming}>{busy?t('studio.saving',{defaultValue:'Saving…'}):t('studio.saveFeedback',{defaultValue:'Save feedback'})}</Button></Flex>
    </form>}
  </Dialog.Content></Dialog.Root>;
}
