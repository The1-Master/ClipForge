import { Button, Flex, Select } from '@radix-ui/themes';
import { useState } from 'react';
import type { ParsedDemo } from '../../api';
import { clock } from '../../api';
import type { AutomationStatus, ReviewMode } from '../../api_automation';
import { displayPlayerName } from '../../playerName';

export function ReviewControls({parsed,player,onPlayer,review,busy,onReview}:{parsed:ParsedDemo;player:string;onPlayer:(player:string)=>void;review?:AutomationStatus['review'];busy?:boolean;onReview?:(playerId:string,mode:ReviewMode)=>void}) {
  const [mode,setMode]=useState<ReviewMode>('targeted');
  const hasPlayer=player!=='all'&&parsed.stats.some(value=>value.steamid===player);
  return <section className="community-review-controls"><Flex gap="3" align="center" wrap="wrap"><Select.Root value={player} onValueChange={onPlayer}><Select.Trigger aria-label="AI review player"/><Select.Content><Select.Item value="all">Choose a player for review</Select.Item>{parsed.stats.map(value=><Select.Item key={value.steamid} value={value.steamid}>{displayPlayerName(value.name)}</Select.Item>)}</Select.Content></Select.Root><Select.Root value={mode} onValueChange={value=>setMode(value as ReviewMode)}><Select.Trigger aria-label="AI review approach"/><Select.Content><Select.Item value="targeted">Quick review — event candidates first</Select.Item><Select.Item value="full-sweep">Full match — candidates + visual sweep</Select.Item></Select.Content></Select.Root>{onReview&&<Button disabled={busy||!hasPlayer} onClick={()=>onReview(player,mode)}>{busy?'Review in progress…':'Review this match'}</Button>}</Flex>
    <p className="studio-caption">{mode==='targeted'?'Quick review inspects event candidates with context. Intervals outside those candidates are not visually reviewed.':'Full review adds a scheduled visual sweep. Deep review remains focused on promising intervals; unavailable intervals are reported.'} Accelerated visual scouting remains disabled until its detection quality is verified.</p>
    {!hasPlayer&&<p>Select a player in this match. You can review a perspective without changing your saved Steam account.</p>}
    {review&&<div className="community-coverage"><strong>Review coverage · {review.mode==='full-sweep'?'Full visual sweep':review.mode==='targeted'?'Targeted candidates':'Legacy review — coverage unknown'}</strong>{review.plan&&<p>{review.plan.candidates} candidates · {review.plan.windows.length} review intervals · {Math.round(review.plan.previewSeconds)}s selected from {Math.round(review.plan.fullMatchSeconds)}s of match play</p>}{review.coverage?.length?<details><summary>Reviewed and skipped intervals ({review.coverage.length})</summary><ul>{review.coverage.map((interval,index)=><li key={`${interval.startTick}:${interval.endTick}:${index}`}>{clock(interval.startTick,parsed.info.tickRate)}–{clock(interval.endTick,parsed.info.tickRate)} · {interval.level} · {interval.status}{interval.reason?' · '+interval.reason:''}</li>)}</ul></details>:<p>No measured interval coverage is available yet. Completion does not imply every moment received deep review.</p>}</div>}
  </section>;
}
