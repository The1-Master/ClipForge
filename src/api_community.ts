import { invoke } from '@tauri-apps/api/core';

export type FeedbackRating = 'good' | 'wrong' | 'missed' | 'timing';
export interface FeedbackSource {
  sourceKind: 'moment' | 'clip' | 'match'; demoId?: string; jobId?: string; outputKey?: string;
  momentId?: string; playerId?: string; startTick?: number; endTick?: number;
  originalProposal?: {title: string; tags: string[]; confidence?: number; startTick?: number; endTick?: number; source?: string};
}
export interface CommunityFeedback extends FeedbackSource {id: string; rating: FeedbackRating; category?: string; notes?: string; createdAt?: string}
export interface Contribution {
  id: string; kind: 'demo' | 'example'; status: string; sentBytes: number; totalBytes: number;
  preview: {files: {name: string; sizeBytes: number}[]; categories: string[]; containsVoice: boolean; containsChat: boolean; description: string};
  error?: string; remoteId?: string; receipt?: string | {id:string;state?:string;submittedAt?:number;rawRetentionDays?:number}; packagePath?: string;
}
export interface ModelRelease {id: string; kind?:'preference-logistic-v1'|'qwen3-vl-lora-v1'|string; model?: string; status?: string; passed?: boolean; active?: boolean; previous?:string; message?: string; metrics?: Record<string, number | string | null>}
export interface CommunityStatus {
  schemaVersion: number;
  settings: {endpoint: string; sharingEnabled: boolean; speechEnabled: boolean};
  feedback: CommunityFeedback[]; contributions: Contribution[];
  operator?: {configured: boolean; role: string};
  training?: {state: string; model: string; teacher: string; examples: number; verifiedExamples: number; versions: ModelRelease[]; message?: string};
  activity?: {busy: boolean; task?: string; phase?: string; progress?: string; startedAt?: string; completed?: number; total?: number; cancelling?: boolean; error?: string};
}
export interface ReviewerExample {id: string; title?: string; category?: string; notes?: string; status?: string; mediaAvailable?: boolean; teacherLabel?: string; groundTruth?: string}
export const communityApi = {
  action: <T = CommunityStatus>(action: string, values: Record<string, unknown> = {}) => invoke<T>('community_action', {action, values}),
  status: () => invoke<CommunityStatus>('community_action', {action: 'status', values: {}}),
};

/** One app-level read in flight, including forced refreshes after mutations. */
export function createCommunityPoller(read: () => Promise<CommunityStatus>, publish: (value: CommunityStatus) => void, fail: (error: unknown) => void) {
  let disposed = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let pending: Promise<void> | undefined;
  function refresh(): Promise<void> {
    if (disposed) return Promise.resolve();
    if (pending) return pending;
    clearTimeout(timer);
    pending = read().then(value => {if (!disposed) publish(value);}).catch(error => {if (!disposed) fail(error);}).finally(() => {
      pending = undefined;
      if (!disposed) timer = setTimeout(() => {void refresh();}, 1500);
    });
    return pending;
  }
  return {refresh, dispose() {disposed = true; clearTimeout(timer);}};
}
