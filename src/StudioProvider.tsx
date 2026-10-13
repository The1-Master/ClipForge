import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import { api, errorText, type AnalysisJob, type DemoMeta, type RenderJob, type SettingsResponse, type Status } from './api';
import { automationApi, type AutomationStatus } from './api_automation';
import { communityApi, createCommunityPoller, type CommunityStatus } from './api_community';
import { createAppSync } from './appSync';
import { createAutomationPoller } from './studioState';
import i18n, { applyLanguage } from './i18n';
import { useNotify } from './components/Notifications';

type Setup = SettingsResponse['setup'];
interface StudioState {
  demos: DemoMeta[]; jobs: RenderJob[]; analysisJobs: AnalysisJob[]; status?: Status;
  setup: Setup; automation?: AutomationStatus; settings?: SettingsResponse; error?: string;
  refresh: () => Promise<void>; rescan: () => Promise<boolean>;
  refreshAutomation: (force?:boolean) => Promise<void>; publishAutomation: (value: AutomationStatus) => void;
  community?: CommunityStatus; communityError?: string; refreshCommunity: () => Promise<void>;
  run: (operation: () => Promise<unknown>) => Promise<boolean>;
}
const Context = createContext<StudioState | null>(null);
let bootstrapStatus: Promise<AutomationStatus> | undefined;
let bootstrapCommunityStatus: Promise<CommunityStatus> | undefined;
export function StudioProvider({ children }: { children: ReactNode }) {
  const notify = useNotify();
  const [demos, setDemos] = useState<DemoMeta[]>([]);
  const [jobs, setJobs] = useState<RenderJob[]>([]);
  const [analysisJobs, setAnalysisJobs] = useState<AnalysisJob[]>([]);
  const [status, setStatus] = useState<Status>();
  const [setup, setSetup] = useState<Setup>({});
  const [settings, setSettings] = useState<SettingsResponse>();
  const [automation, setAutomation] = useState<AutomationStatus>();
  const [community, setCommunity] = useState<CommunityStatus>();
  const [communityError, setCommunityError] = useState<string>();
  const [error, setError] = useState<string>();
  const syncRef = useRef<ReturnType<typeof createAppSync> | undefined>(undefined);
  const pollRef = useRef<ReturnType<typeof createAutomationPoller> | undefined>(undefined);
  const communityPollRef = useRef<ReturnType<typeof createCommunityPoller> | undefined>(undefined);
  const rescan = useCallback(() => syncRef.current?.refresh() ?? Promise.resolve(false), []);
  const refreshAutomation = useCallback((force=false) => pollRef.current?.refresh(force) ?? Promise.resolve(), []);
  const refreshCommunity = useCallback(() => communityPollRef.current?.refresh() ?? Promise.resolve(), []);
  const refresh = useCallback(async () => {
    await Promise.all([rescan(), refreshAutomation(), refreshCommunity(), api.settings().then(value => { setSettings(value); setSetup(value.setup); })]);
  }, [rescan, refreshAutomation, refreshCommunity]);
  const run = useCallback(async (operation: () => Promise<unknown>) => {
    try { await operation(); await refresh(); return true; }
    catch (e) { notify(errorText(e)); return false; }
  }, [notify, refresh]);
  useEffect(() => {
    let alive = true;
    const upsert = <T,>(list: T[], item: T, key: (value: T) => string) => {
      const index=list.findIndex(value=>key(value)===key(item));
      if(index<0)return [...list,item];
      const next=[...list];next[index]=item;return next;
    };
    const observedJobs = new Map<string,RenderJob['status']>();
    const sync = createAppSync(api, snapshot => {
      observedJobs.clear();
      for (const job of snapshot.jobs) observedJobs.set(job.id,job.status);
      setDemos(snapshot.demos); setJobs(snapshot.jobs); setAnalysisJobs(snapshot.analysisJobs); setStatus(snapshot.status); setError(undefined);
    }, event => {
      if (event.type === 'demo-changed') setDemos(list => upsert(list, event.demo, value => value.id));
      if (event.type === 'job-changed') {
        const previous = observedJobs.get(event.job.id);
        observedJobs.set(event.job.id,event.job.status);
        if (previous !== event.job.status && event.job.status === 'error') notify(i18n.t('studio.exportNeedsAttention',{defaultValue:"An export needs attention. Open Activity \u2192 History for details."}));
        if (previous !== event.job.status && event.job.status === 'partial') notify(i18n.t('studio.exportPartialAttention',{defaultValue:"Some moments could not be exported. Completed clips are saved; open Activity \u2192 History for details."}),'amber');
        setJobs(list => upsert(list, event.job, value => value.id).sort((a,b) => b.createdAt.localeCompare(a.createdAt)));
      }
      if (event.type === 'analysis-job-changed') setAnalysisJobs(list => {
        const old = list.find(value => value.id === event.job.id);
        return old && old.revision > event.job.revision ? list : upsert(list, event.job, value => value.id).sort((a,b) => b.sequence-a.sequence);
      });
      if (event.type === 'setup-progress') setSetup(value => ({...value,[event.tool]:{running:true,log:[],progress:event.progress}}));
      if (event.type === 'setup-finished') {
        setSetup(value => ({...value,[event.tool]:undefined}));
        if (!event.ok && !event.cancelled) notify(event.error ?? i18n.t('studio.toolInstallationAttention',{defaultValue:"Tool installation needs attention."}));
      }
    }, e => setError(errorText(e)));
    syncRef.current = sync;
    // Share only the initial subprocess refresh across StrictMode remounts.
    let initial=true;
    const poll = createAutomationPoller(force => {
      if(force&&initial){
        initial=false;
        if(!bootstrapStatus){
          const request=automationApi.status(true);
          bootstrapStatus=request;
          void request.finally(()=>{if(bootstrapStatus===request)bootstrapStatus=undefined;}).catch(()=>undefined);
        }
        return bootstrapStatus;
      }
      return automationApi.status(force);
    }, setAutomation, e => setError(errorText(e)));
    pollRef.current = poll;
    let communityInitial=true;
    const communityPoll = createCommunityPoller(() => {
      if(communityInitial){
        communityInitial=false;
        if(!bootstrapCommunityStatus){
          const request=communityApi.status();bootstrapCommunityStatus=request;
          void request.finally(()=>{if(bootstrapCommunityStatus===request)bootstrapCommunityStatus=undefined;}).catch(()=>undefined);
        }
        return bootstrapCommunityStatus;
      }
      return communityApi.status();
    }, value => {setCommunity(value); setCommunityError(undefined);}, e => setCommunityError(errorText(e)));
    communityPollRef.current = communityPoll;
    void sync.refresh();
    void poll.refresh(true);
    void communityPoll.refresh();
    void api.settings().then(value => { if (alive) { setSettings(value); setSetup(current => ({...value.setup,...current})); applyLanguage(value.settings.language); } }).catch(e => { if (alive) setError(errorText(e)); });
    return () => { alive = false; sync.dispose(); poll.dispose(); communityPoll.dispose(); syncRef.current = undefined; pollRef.current = undefined; communityPollRef.current = undefined; };
  }, [notify]);
  return <Context.Provider value={{demos,jobs,analysisJobs,status,setup,settings,automation,community,communityError,refreshCommunity,error,refresh,rescan,refreshAutomation,publishAutomation:setAutomation,run}}>{children}</Context.Provider>;
}
export function useStudio() {
  const state = useContext(Context);
  if (!state) throw new Error('StudioProvider is required');
  return state;
}
/** Reusable replay/feedback controls also run in the isolated component workbench. */
export function useOptionalStudio() {return useContext(Context);}
