import type { AutomationStatus } from './api_automation';

// One cached status read at a time; route changes never create new pollers.
export function createAutomationPoller(read: (refresh: boolean) => Promise<AutomationStatus>, publish: (value: AutomationStatus) => void, fail: (error: unknown) => void) {
  let disposed = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let pending: Promise<void> | undefined;
  let activeForce=false;
  let queuedForce:Promise<void>|undefined;
  function refresh(force = false): Promise<void> {
    if (disposed) return Promise.resolve();
    if (pending) {
      if(!force||activeForce)return pending;
      queuedForce??=pending.then(()=>refresh(true)).finally(()=>{queuedForce=undefined;});
      return queuedForce;
    }
    clearTimeout(timer);
    activeForce=force;
    pending = read(force).then(value => { if (!disposed) publish(value); }).catch(error => { if (!disposed) fail(error); }).finally(() => {
      pending = undefined;
      if (!disposed) timer = setTimeout(() => { void refresh(); }, 1500);
    });
    return pending;
  }
  return { refresh, dispose() { disposed = true; clearTimeout(timer); } };
}

export type StudioRoute = { page: 'home'|'clips'|'matches'|'activity'|'settings'; demoId?: string; tab?: string; clipKey?: string; section?: SettingsSection };
export type SettingsSection = 'connection'|'automation'|'ai'|'community'|'recording'|'appearance'|'tools'|'storage'|'about';
export interface DraftGuard { dirty: boolean; save: () => Promise<boolean>; discard: () => void }
