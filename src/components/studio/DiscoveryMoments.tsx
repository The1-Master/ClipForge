import { useEffect, useMemo, useState } from 'react';
import { Badge, Box, Button, Checkbox, Flex, Select, Text } from '@radix-ui/themes';
import { clock, type DemoMeta, type ParsedDemo, type Status } from '../../api';
import { automationApi } from '../../api_automation';
import { FeedbackButton } from './FeedbackDialog';
import { ExportDialog } from '../ExportDialog';
import { displayPlayerName } from '../../playerName';

const categoryLabels: Record<string,string> = {all:'All discoveries',whiff_candidate:'Extreme whiff candidates',exceptional_shot:'Exceptional shots',combat:'Combat sequences',death:'Deaths',utility_mistake:'Utility mistakes',tension:'Tense moments'};
const filters = new Map<string,{player:string;category:string}>();
export function DiscoveryMoments({meta,parsed,playerId,status,onSetup,onRendered}:{meta:DemoMeta;parsed:ParsedDemo;playerId?:string;status?:Status;onSetup:()=>void;onRendered:()=>void}) {
  const preferred = parsed.stats.some(player=>player.steamid===playerId)?playerId!:'all';
  const [player,setPlayer]=useState(filters.get(meta.id)?.player??preferred);
  const [category,setCategory]=useState(filters.get(meta.id)?.category??'all');
  const [selected,setSelected]=useState(new Set<string>());
  const [exporting,setExporting]=useState(false);
  const candidates=parsed.discoveryCandidates??[];
  useEffect(()=>{filters.set(meta.id,{player,category});},[meta.id,player,category]);
  useEffect(()=>{if(player!=='all'&&!parsed.stats.some(value=>value.steamid===player))setPlayer(preferred);},[parsed.stats,player,preferred]);
  const visible=useMemo(()=>candidates.filter(candidate=>(player==='all'||candidate.playerId===player)&&(category==='all'||candidate.category===category)),[candidates,player,category]);
  const chosen=candidates.filter(candidate=>selected.has(candidate.id));
  const allSelected=visible.length>0&&visible.every(candidate=>selected.has(candidate.id));
  const canExport=chosen.length>0&&player!=='all'&&chosen.every(candidate=>candidate.playerId===player);
  const tr=parsed.info.tickRate;
  return <Box><h3>Whiffs, exceptional shots & overlooked moments</h3><p>Fast demo analysis nominates these intervals without watching the entire match. A miss is a candidate, not proof of a whiff: review its context before keeping it.</p>
    <Flex gap="3" wrap="wrap" align="center" mb="3"><Select.Root value={player} onValueChange={value=>{setPlayer(value);setSelected(new Set());}}><Select.Trigger aria-label="Discovery player"/><Select.Content><Select.Item value="all">All players — choose one to export</Select.Item>{parsed.stats.map(value=><Select.Item value={value.steamid} key={value.steamid}>{displayPlayerName(value.name)}</Select.Item>)}</Select.Content></Select.Root><Select.Root value={category} onValueChange={setCategory}><Select.Trigger aria-label="Discovery category"/><Select.Content>{Object.entries(categoryLabels).map(([value,label])=><Select.Item key={value} value={value}>{label}</Select.Item>)}</Select.Content></Select.Root><Button variant="soft" onClick={()=>setSelected(current=>{const next=new Set(current);for(const candidate of visible)allSelected?next.delete(candidate.id):next.add(candidate.id);return next;})}>{allSelected?'Deselect shown':'Select shown'}</Button><Button disabled={!canExport} onClick={()=>setExporting(true)}>Export {chosen.length||''} selected discoveries</Button></Flex>
    {player==='all'&&<p className="studio-caption">Choose the player perspective to export. Every recorded personal death remains available in Deaths.</p>}
    {!candidates.length&&<p>No discovery ledger is available for this match yet. Reparse an older demo to create it.</p>}
    {candidates.length>0&&!visible.length&&<p>No discoveries match these filters.</p>}
    {visible.map(candidate=><section key={candidate.id} className="studio-ai-suggestion"><Flex align="center" gap="3" wrap="wrap"><Checkbox aria-label={`Select ${candidate.title}`} checked={selected.has(candidate.id)} onCheckedChange={()=>setSelected(current=>{const next=new Set(current);next.has(candidate.id)?next.delete(candidate.id):next.add(candidate.id);return next;})}/><strong>{candidate.title}</strong><Badge color="gray">{categoryLabels[candidate.category]}</Badge><Text size="2">R{candidate.round} · {clock(candidate.startTick,tr)} · {Math.round((candidate.endTick-candidate.startTick)/tr)}s</Text></Flex><p>{displayPlayerName(parsed.stats.find(value=>value.steamid===candidate.playerId)?.name)} · {candidate.requiresVisualReview?'Needs visual/context review':'Demo event evidence'}</p><Flex gap="1" wrap="wrap">{candidate.tags.map(tag=><Badge key={tag} color="gray">{tag}</Badge>)}</Flex><details className="studio-evidence"><summary>Event evidence</summary><ul>{candidate.evidence.map((evidence,index)=><li key={evidence.eventId+':'+index}>{clock(evidence.tick,tr)} · {evidence.kind}{evidence.weapon?' · '+evidence.weapon:''}{evidence.value!==undefined?' · '+evidence.value:''}</li>)}</ul><p>Detected by: {candidate.origins.join(', ')}</p></details><Flex mt="3"><FeedbackButton tickRate={tr} source={{sourceKind:'moment',demoId:meta.id,momentId:candidate.id,playerId:candidate.playerId,startTick:candidate.startTick,endTick:candidate.endTick,originalProposal:{title:candidate.title,tags:candidate.tags,startTick:candidate.startTick,endTick:candidate.endTick,source:'event-ledger'}}}/></Flex></section>)}
    <ExportDialog open={exporting} onOpenChange={setExporting} count={chosen.length} seconds={chosen.reduce((sum,candidate)=>sum+(candidate.endTick-candidate.startTick)/tr,0)} status={status} onSetup={onSetup} disabled={!canExport} onSubmit={options=>automationApi.custom(meta.id,player,chosen.map(candidate=>[candidate.startTick,candidate.endTick]),chosen.length===1?chosen[0]!.title:'Discovery moments',{...options,keyMomentsOnly:false},'custom',chosen.map(candidate=>candidate.id))} onSubmitted={()=>{setSelected(new Set());onRendered();}}/>
  </Box>;
}
