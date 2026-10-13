import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Badge, Button, Checkbox, Flex, Select, Text, TextField } from '@radix-ui/themes';
import { errorText, type DemoMeta } from '../api';
import { automationApi, type AutomationSettings } from '../api_automation';
import { useStudio } from '../StudioProvider';
import type { DraftGuard } from '../studioState';
import { Icon } from './studio/Icon';

export function AutomationView({section='connection',onGuardChange}: {section?:'connection'|'automation'|'ai';demos?:DemoMeta[];onChanged?:()=>void;onGuardChange?:(guard:DraftGuard|undefined)=>void}) {
  const {t}=useTranslation();
  const studio=useStudio();
  const state=studio.automation;
  const [draft,setDraft]=useState<AutomationSettings>();
  const [apiKey,setApiKey]=useState('');
  const [authCode,setAuthCode]=useState('');
  const [error,setError]=useState<string>();
  const [saving,setSaving]=useState(false);
  const [dirty,setDirty]=useState(false);
  const dirtyFields=useRef(new Set<keyof AutomationSettings>());
  useEffect(()=>{if(!dirty&&state?.settings)setDraft(state.settings);},[state?.settings,dirty]);
  const change=<K extends keyof AutomationSettings>(key:K,value:AutomationSettings[K])=>{
    dirtyFields.current.add(key);setDraft(current=>current?{...current,[key]:value}:current);setDirty(true);
  };
  const discard=()=>{setDraft(state?.settings);setApiKey('');setAuthCode('');dirtyFields.current.clear();setDirty(false);setError(undefined);};
  const save=async()=>{
    if(!draft||saving)return false;
    setSaving(true);setError(undefined);
    try{
      const values=Object.fromEntries([...dirtyFields.current].map(key=>[key,draft[key]]));
      const next=await automationApi.save({...values,...(apiKey?{apiKey}:{}),...(authCode?{authCode}:{})});
      studio.publishAutomation(next);setDraft(next.settings);setApiKey('');setAuthCode('');dirtyFields.current.clear();setDirty(false);await studio.refresh();return true;
    }catch(e){setError(errorText(e));return false;}finally{setSaving(false);}
  };
  useEffect(()=>{onGuardChange?.({dirty,save,discard});return()=>onGuardChange?.(undefined);},[dirty,draft,apiKey,authCode,saving,state?.settings,onGuardChange]);
  if(!draft)return <section className="studio-card"><h2>Connecting to your local settings</h2><p>{studio.error||'Loading saved match-history configuration…'}</p><Button variant="soft" onClick={()=>void studio.refreshAutomation(true)}>Refresh connection</Button></section>;
  const busy=state?.activity.busy||state?.review?.complete===false;
  const matchHistoryAvailable=state?.capabilities?.matchHistory!==false;
  const modelBadge=(present:boolean|undefined,manifest=false)=><Badge color={present===true?'green':present===false?'amber':'gray'}>{present===undefined?t('studio.modelNotChecked',{defaultValue:'Not checked'}):present===false?t('studio.modelMissing',{defaultValue:'Missing'}):manifest?t('studio.modelManifestPresent',{defaultValue:'Manifest present'}):t('studio.modelFilePresent',{defaultValue:'File present'})}</Badge>;
  return <fieldset disabled={saving} className="studio-settings-form" style={{border:0,padding:0,margin:0,minWidth:0}}>
    <section hidden={section!=='connection'} className="studio-card"><span className="studio-eyebrow">STEAM CONNECTION</span><h2>Your matches, in your studio</h2><p>{matchHistoryAvailable?t('studio.matchHistoryConnectionDescription',{defaultValue:'Use your Steam match-history access to download available replays. Saved credentials stay protected for your Windows account.'}):t('studio.manualImportConnectionDescription',{defaultValue:'Import a saved CS2 demo from Home or Matches to start finding clips. Match-history downloads are unavailable in this installation.'})}</p>
      <Flex gap="2" wrap="wrap" mb="4"><Badge color={matchHistoryAvailable&&draft.authCodeSet?'green':'gray'}>{!matchHistoryAvailable?t('studio.manualDemoImport',{defaultValue:'Manual demo import'}):draft.authCodeSet?'Authentication saved':'Recent-match discovery available'}</Badge><Badge>{state?.matches?.filter(m=>m.path).length??0} downloaded matches</Badge></Flex>
      <div className="studio-form-grid"><label>Steam ID64<TextField.Root aria-label="Steam ID64" value={draft.steamId} onChange={e=>change('steamId',e.target.value)} placeholder="Your 17-digit Steam ID" autoComplete="off"/></label>
      <label>Recent match sharing code<TextField.Root aria-label="Recent match sharing code" value={draft.lastShareCode} onChange={e=>change('lastShareCode',e.target.value)} placeholder="CSGO-…" autoComplete="off"/></label>
      <label>Match-history authentication code {draft.authCodeSet&&<Badge color="green">Saved</Badge>}<TextField.Root aria-label="Match-history authentication code" type="password" value={authCode} onChange={e=>{setAuthCode(e.target.value);setDirty(true);}} autoComplete="off"/></label>
      <label>Steam Web API key · optional {draft.apiKeySet&&<Badge color="green">Saved</Badge>}<TextField.Root aria-label="Steam Web API key" type="password" value={apiKey} onChange={e=>{setApiKey(e.target.value);setDirty(true);}} autoComplete="off"/></label></div>
      <p className="studio-caption">{matchHistoryAvailable?t('studio.matchHistoryCredentialsHint',{defaultValue:'Leave credential fields empty to keep their saved values. Steam can supply available recent matches without a Web API key.'}):t('studio.manualImportCredentialsHint',{defaultValue:'Saved connection details stay protected for your Windows account. Leave credential fields empty to keep their saved values.'})}</p>
      <Button disabled={!matchHistoryAvailable||busy||dirty} onClick={()=>{if(!matchHistoryAvailable)return;void studio.run(()=>automationApi.action({command:'sync'}));}}><Icon name="download"/>Download my matches</Button>
    </section>
    <section hidden={section!=='automation'} className="studio-card"><span className="studio-eyebrow">YOUR RECORDING RULES</span><h2>Choose what happens automatically</h2><p>ClipForge waits for your normal CS2 session to close before recording or running local AI.</p>
      <div className="studio-choice-list"><label><Checkbox checked={draft.enabled} onCheckedChange={v=>change('enabled',v===true)}/><span><strong>Automatically process new matches</strong><small>Starts after CS2 closes. Currently {state?.settings?.enabled?'on':'off'}.</small></span></label>
      <label><Checkbox checked={draft.autoExport} onCheckedChange={v=>change('autoExport',v===true)}/><span><strong>Export detected moments</strong><small>Use your saved recording quality for new exports.</small></span></label>
      <label><Checkbox checked={draft.saveDeaths} onCheckedChange={v=>change('saveDeaths',v===true)}/><span><strong>Keep every personal death</strong><small>Deaths stay separate from highlight-score filtering.</small></span></label>
      <label><Checkbox checked={draft.aiEnabled} onCheckedChange={v=>change('aiEnabled',v===true)}/><span><strong>Look for funny fails and bloopers</strong><small>Review locally using the AI model chosen in AI settings.</small></span></label></div>
      <div className="studio-form-grid"><label>Multi-kill minimum<Select.Root value={String(draft.minKills)} onValueChange={v=>change('minKills',Number(v))}><Select.Trigger aria-label="Multi-kill minimum"/><Select.Content>{[1,2,3,4,5].map(n=><Select.Item value={String(n)} key={n}>{n} kills</Select.Item>)}</Select.Content></Select.Root></label>
      <label>AI confidence threshold<TextField.Root aria-label="AI confidence threshold" type="number" min={0} max={100} value={draft.aiThreshold} onChange={e=>change('aiThreshold',Number(e.target.value))}/></label></div>
    </section>
    <section hidden={section!=='ai'} className="studio-card"><span className="studio-eyebrow">LOCAL AI</span><h2>Find the unexpected moments</h2><p>AI blooper review uses measured frames from your own gameplay and keeps evidence for suggestions. Funny moments are subjective, and full-match review can take time.</p>
      <div className="studio-form-grid"><label>Vision model<Select.Root value={draft.model} onValueChange={v=>change('model',v)}><Select.Trigger aria-label="Vision model"/><Select.Content><Select.Item value="qwen3-vl:8b-instruct">Larger model · 8B</Select.Item><Select.Item value="qwen3-vl:4b-instruct">Lighter model · 4B</Select.Item>{state?.models?.models?.filter(model=>model.name.startsWith('clipforge-qwen3-vl:')).map(model=><Select.Item key={model.name} value={model.name}>Evaluated project model · {model.name.split(':')[1]?.slice(0,8)}</Select.Item>)}</Select.Content></Select.Root></label></div>
      <div className="studio-model-status"><div data-model="runtime"><span>Ollama {state?.models?.version??''}</span>{modelBadge(state?.models?.runtimeInstalled)}</div>{['qwen3-vl:8b-instruct','qwen3-vl:4b-instruct'].map(name=><div key={name} data-model={name}><span>{name}</span>{modelBadge(state?.models?.models?.find(model=>model.name===name)?.installed,true)}</div>)}<div data-model="speech"><span>Whisper small</span>{modelBadge(state?.models?.speechInstalled,true)}</div></div>
      <p className="studio-caption">{t('studio.modelPresenceHint',{defaultValue:'These statuses report local files and manifests. GPU inference is checked separately when a review runs.'})}</p>
      <p>Start a review from a match’s Moments screen. The review first records a lightweight preview, then offers selected moments at your export quality.</p>
      <p className="studio-caption">Voice reactions require recorded voice chat in the demo. Missing conversations cannot be recovered.</p>
      <Button variant="soft" disabled={busy} onClick={()=>void studio.run(()=>automationApi.action({command:'install-models'}))}><Icon name="download"/>Install / check local AI models</Button>
    </section>
    {error&&<Text as="p" color="red" role="alert">{error}</Text>}
    <Flex gap="3" align="center" wrap="wrap" className="studio-save-bar"><Button disabled={!dirty||saving} onClick={()=>void save()}>{saving?'Saving…':'Save changes'}</Button>{dirty&&<><Button variant="ghost" disabled={saving} onClick={discard}>Discard changes</Button><Text color="amber">Unsaved changes</Text></>}</Flex>
  </fieldset>;
}
