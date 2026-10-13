/** Private Vite test entry. Never imported by the application or a production route. */
import React, { useCallback, useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Button, Flex, IconButton, Popover, Text, Tooltip } from '@radix-ui/themes';
import { useTranslation } from 'react-i18next';
import { NotificationProvider, Toast, useNotify } from '../../src/components/Notifications';
import { AppTheme, useAppTheme } from '../../src/AppTheme';
import { api, errorText, type AnalysisJob, type DemoMeta, type RenderJob, type SettingsResponse, type Status } from '../../src/api';
import { createAppSync } from '../../src/appSync';
import { applyLanguage } from '../../src/i18n';
import { StartupGate } from '../../src/components/StartupGate';
import { DemoList } from '../../src/components/DemoList';
import { DemoView } from '../../src/components/DemoView';
import { SettingsView } from '../../src/components/SettingsView';
import { ToolInstallDialog } from '../../src/components/ToolInstallDialog';
import { AboutDialog } from '../../src/components/AboutDialog';
import { QueueDialog } from '../../src/components/QueueDialog';
import '@radix-ui/themes/styles.css';
import 'react-day-picker/style.css';
import 'bootstrap-icons/font/bootstrap-icons.css';
import '../../src/styles.css';
import './component-workbench.css';

function Workbench() {
  const { t } = useTranslation();
  const notify = useNotify();
  const theme = useAppTheme();
  const [screen, setScreen] = useState<'match' | 'settings'>('match');
  const [toolbar, setToolbar] = useState<HTMLDivElement | null>(null);
  const [fontSizeOpen, setFontSizeOpen] = useState(false);
  const [selectionRequest, setSelectionRequest] = useState(0);
  const [tab, setTab] = useState('moments');
  const [selectedId, setSelectedId] = useState<string>();
  const [setup, setSetup] = useState<SettingsResponse['setup']>({});
  const [status, setStatus] = useState<Status>();
  const [demos, setDemos] = useState<DemoMeta[]>([]);
  const [jobs, setJobs] = useState<RenderJob[]>([]);
  const [analysisJobs, setAnalysisJobs] = useState<AnalysisJob[]>([]);
  const [toolsRequest, setToolsRequest] = useState(0);
  const [toolsTarget, setToolsTarget] = useState<'render' | 'replay'>('render');
  const [error, setError] = useState<string>();
  const syncRef = useRef<ReturnType<typeof createAppSync> | undefined>(undefined);
  const rescan = useCallback(() => syncRef.current?.refresh() ?? Promise.resolve(false), []);
  const refresh = useCallback(async () => { await rescan(); }, [rescan]);
  useEffect(() => {
    const upsert = <T,>(list: T[], item: T, key: (value: T) => string) => { const index = list.findIndex(value => key(value) === key(item)); if (index < 0) return [item, ...list]; const next = list.slice(); next[index] = item; return next; };
    const sync = createAppSync(api, snapshot => {
      setStatus(snapshot.status); setDemos(snapshot.demos); setJobs(snapshot.jobs); setAnalysisJobs(snapshot.analysisJobs); setError(undefined);
    }, event => {
      if (event.type === 'demo-changed') setDemos(list => upsert(list, event.demo, value => value.id));
      if (event.type === 'job-changed') setJobs(list => upsert(list, event.job, value => value.id).sort((a,b) => b.createdAt.localeCompare(a.createdAt)));
      if (event.type === 'analysis-job-changed') setAnalysisJobs(list => { const old = list.find(job => job.id === event.job.id); return old && old.revision > event.job.revision ? list : upsert(list, event.job, value => value.id).sort((a,b) => b.sequence-a.sequence); });
      if (event.type === 'setup-progress') setSetup(current => ({ ...current, [event.tool]: { running: true, log: [], progress: event.progress } }));
      if (event.type === 'setup-finished') { setSetup(current => ({ ...current, [event.tool]: undefined })); if (!event.ok && !event.cancelled) notify(event.error ?? t('settings.unknownError')); }
    }, reason => setError(errorText(reason)));
    syncRef.current = sync;
    void sync.refresh().then(() => api.settings()).then(value => { applyLanguage(value.settings.language); setSetup(current => ({ ...value.setup, ...current })); }).catch(() => undefined);
    return () => { syncRef.current = undefined; sync.dispose(); };
  }, [notify]);
  const selected = demos.find(demo => demo.id === selectedId);
  const showTools = (target: 'render' | 'replay') => { setToolsTarget(target); setScreen('settings'); setToolsRequest(value => value + 1); };
  return <div className="layout component-workbench"><ToolInstallDialog setup={setup} /><header className="app-header"><div ref={setToolbar} className="header-tools" /><Flex align="center" gap="2" ml="auto">
    <QueueDialog jobs={jobs} analysisJobs={analysisJobs} demos={demos} onChanged={refresh} onSelect={(id, selectedTab) => { setSelectedId(id); setScreen('match'); setTab(selectedTab); setSelectionRequest(value => value + 1); }} />
    <Tooltip delayDuration={150} content={t(theme.appearance === 'dark' ? 'ui.light' : 'ui.dark')}><IconButton variant="ghost" aria-label={t(theme.appearance === 'dark' ? 'ui.light' : 'ui.dark')} onClick={theme.toggle}><i aria-hidden="true" className={theme.appearance === 'dark' ? 'bi bi-sun app-icon' : 'bi bi-moon app-icon'} /></IconButton></Tooltip>
    <Popover.Root open={fontSizeOpen} onOpenChange={setFontSizeOpen}><Popover.Trigger><Button variant="ghost" aria-label={t('ui.fontSize')}>Aa</Button></Popover.Trigger><Popover.Content size="1"><div className="font-size-control" role="radiogroup" aria-label={t('ui.fontSize')}>{(['small', 'medium', 'large'] as const).map((size, index) => <label key={size}><input type="radio" name="font-size" value={size} checked={theme.fontSize === size} onClick={() => setFontSizeOpen(false)} onChange={() => theme.setFontSize(size)} aria-label={t(`ui.fontSize_${size}`)} /><span aria-hidden="true" style={{ fontSize: 14 + index * 3 }}>Aa</span></label>)}</div></Popover.Content></Popover.Root>
    <Button variant="ghost" aria-pressed={screen === 'settings'} onClick={() => { setToolsRequest(0); setScreen(current => current === 'settings' ? 'match' : 'settings'); }}>{t('common.settings')}</Button><AboutDialog triggerLabel="About" />
  </Flex></header><aside className="sidebar"><DemoList demos={demos} jobs={jobs} selectedId={screen === 'match' ? selectedId : undefined} toolbar={toolbar} selectionRequest={selectionRequest} onSelect={id => { setSelectedId(id); setScreen('match'); setSelectionRequest(value => value + 1); }} onChanged={refresh} onRefresh={rescan} /></aside>
    {error && <Toast message={t('app.backendError', { error })} color="red" onDismiss={() => setError(undefined)} action={<Button variant="soft" onClick={() => setScreen('settings')}>{t('common.settings')}</Button>} />}
    <main className="main">{screen === 'settings' ? <SettingsView onChanged={refresh} group="all" toolsRequest={toolsRequest} toolsTarget={toolsTarget} /> : selected ? <DemoView key={selected.id} meta={selected} jobs={jobs.filter(job => job.demoId === selected.id)} status={status} analysisJobs={analysisJobs} tab={tab} setTab={setTab} onChanged={refresh} onRemoved={() => setSelectedId(undefined)} onSetup={showTools} /> : <Flex align="center" justify="center" style={{ height: '100%' }}><Text color="gray">{t('app.pickDemo')}</Text></Flex>}</main>
  </div>;
}

createRoot(document.getElementById('root')!).render(<React.StrictMode><AppTheme><NotificationProvider><StartupGate><Workbench /></StartupGate></NotificationProvider></AppTheme></React.StrictMode>);
