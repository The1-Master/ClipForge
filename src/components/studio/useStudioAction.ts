import { useEffect, useRef, useSyncExternalStore } from 'react';
import { api, errorText, type RenderJob } from '../../api';
import { useStudio } from '../../StudioProvider';
import { useNotify } from '../Notifications';
import i18n from '../../i18n';
import type { StudioRoute } from '../../studioState';

const locks = new Set<string>();
let pendingSnapshot = new Set<string>();
const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
const getSnapshot = () => pendingSnapshot;
const publish = () => { pendingSnapshot = new Set(locks); for (const listener of listeners) listener(); };

/** Immediate session locks survive screen changes until the original action settles. */
export function useStudioAction() {
  const pending = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const run = async (key: string, operation: () => Promise<unknown>) => {
    if (locks.has(key)) return;
    locks.add(key);
    publish();
    try { await operation(); }
    finally { locks.delete(key); publish(); }
  };
  return { pending, run, alive };
}

/** Opening a job source always uses native snapshot validation and its returned ID. */
export function useRenderSource(onNavigate: (route: StudioRoute) => void) {
  const state = useStudio();
  const notify = useNotify();
  const action = useStudioAction();
  const open = (job: RenderJob, tab = 'renders') => action.run(job.id, async () => {
    try {
      const demo = await api.openRenderSource(job.id);
      if (!await state.rescan()) throw new Error(i18n.t('studio.sourceLibraryRefreshRecovery',{defaultValue:"The source was verified, but the match library could not refresh. Try opening it again."}));
      if (action.alive.current) onNavigate({ page: 'matches', demoId: demo.id, tab });
    } catch (error) { notify(errorText(error)); }
  });
  return { open, pending: action.pending };
}
