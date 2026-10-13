import { invoke } from '@tauri-apps/api/core';
import type { RenderJob, RenderOptions } from './api.ts';

export interface AutomationSettings {
  steamId: string; lastShareCode: string; enabled: boolean; autoExport: boolean;
  saveDeaths: boolean; minKills: number; aiEnabled: boolean; aiThreshold: number;
  model: string; apiKeySet: boolean; authCodeSet: boolean;
}
export interface FunnyMoment {
  title: string; startTick: number; endTick: number; confidence: number;
  tags: string[]; evidence: unknown; steamId?: string; source?: string; reason?: string; cameraFacts?: CameraFacts;
}
export interface CameraFacts {frameCount:number;livingFrames:number;personalDeathFrames:number;observerMode:2;source:'source2_observer_handle'}
export type ReviewMode = 'targeted'|'full-sweep';
export interface ReviewCoverage {startTick:number;endTick:number;level:'deep'|'coarse'|'unavailable'|'not-reviewed';status:string;reason?:string}
/** Presence reports from native local manifests/files, not inference benchmarks. */
export interface ModelStatus {
  runtimeInstalled?:boolean;version?:string;models?:{name:string;installed:boolean;digest?:string|null}[];
  speechInstalled?:boolean;visionInstalled?:boolean;freeBytes?:number;reserveBytes?:number;localOnly?:boolean;model?:string;
  hardware?:{gpuLoaded?:boolean;tested?:boolean};
}
export interface AutomationStatus {
  capabilities?: {matchHistory:boolean};
  settings?: AutomationSettings;
  matches?: {code:string;status:string;path?:string;error?:string;updated?:number;attempts?:number;nextRetry?:number}[];
  models?: ModelStatus;
  gameOpen: boolean; queuePaused: boolean;
  gamePresence?: 'closed'|'external'|'recording'|'unknown';
  activity: {busy:boolean;task?:string;progress?:string;phase?:string;demoId?:string;completed?:number;total?:number;startedAt?:string|number;cancelling?:boolean;error?:string;result?:{moments?:FunnyMoment[];errors?:string[];cameraFacts?:CameraFacts}};
  review?: {demoId:string;jobId:string;playerId:string;status:string;moments:FunnyMoment[];error?:string;complete?:boolean;nextOutput?:number;model?:string;demoDigest?:string;phase?:string;completed?:number;total?:number;cancelling?:boolean;startedAt?:string;cameraFacts?:CameraFacts;mode?:ReviewMode;coverage?:ReviewCoverage[];plan?:{candidates:number;windows:{round:number;startTick:number;endTick:number;candidateIds:string[]}[];previewSeconds:number;fullMatchSeconds:number;coverageScope:string}};
}
export const automationApi = {
  status: (refresh=false) => invoke<AutomationStatus>('automation_status', {refresh}),
  save: (values: Partial<AutomationSettings> & {apiKey?:string;authCode?:string}) => invoke<AutomationStatus>('automation_save', {values}),
  action: (request: Record<string,unknown>) => invoke<void>('automation_action', {request}),
  cancel: () => invoke<void>('automation_cancel'),
  pause: (paused:boolean) => invoke<void>('pause_queue',{paused}),
  custom: (demoId:string,playerId:string,windows:[number,number][],title:string,options:RenderOptions,origin:'custom'|'blooper'='custom',candidateIds?:string[]) =>
    invoke<RenderJob>('start_custom_render',{demoId,playerId,windows,title,options,origin,...(candidateIds?{candidateIds}:{})}),
  reviewMatch: (demoId:string,playerId:string,mode:ReviewMode='targeted') => invoke<void>('prepare_match_review',{demoId,playerId,mode}),
};
