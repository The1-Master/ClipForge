import { ConfirmDialog, Toast, useNotify } from './Notifications.tsx';
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { Box, Button, Callout, Card, Dialog, Flex, Grid, Heading, IconButton, Select, Switch, Text, TextField, Tooltip } from '@radix-ui/themes';
import { driver } from 'driver.js';
import 'driver.js/dist/driver.css';
import { open } from '@tauri-apps/plugin-dialog';
import { useTranslation } from 'react-i18next';
import { api, errorText, mb, DEFAULT_RENDER_OPTIONS, RENDER_QUALITY, type Settings, type ToolPaths, type ToolUpdate, type SettingsResponse, type StorageBytes } from '../api.ts';
import type { DraftGuard } from '../studioState';
import { LogView } from './LogView.tsx';
import { Spinner } from './Spinner.tsx';
import i18n, { applyLanguage, translateProblem, detectLanguage, LANGUAGE_NAMES, LANGUAGES } from '../i18n/index.ts';

// Preserve legacy executable settings while presenting folder selection in the UI.
const toolDirectory = (value?: string | null) => {
  if (!value || !/[/\\][^/\\]+\.exe$/i.test(value)) return value ?? '';
  const parent = value.replace(/[/\\][^/\\]+$/, '');
  const executable = value.split(/[/\\]/).pop()?.toLowerCase();
  const tool = ({ 'hlae.exe': 'hlae', 'ffmpeg.exe': 'ffmpeg', 'source2viewer-cli.exe': 'vrf' } as Record<string, string>)[executable ?? ''];
  if (tool) {
    let ancestor = parent;
    while (ancestor) {
      const separator = Math.max(ancestor.lastIndexOf('/'), ancestor.lastIndexOf('\\'));
      if (separator < 0) break;
      if (ancestor.slice(separator + 1).toLowerCase() === tool) {
        return ancestor.slice(0, separator === 2 && ancestor[1] === ':' ? 3 : separator);
      }
      ancestor = ancestor.slice(0, separator);
    }
  }
  return parent;
};

function toolsInstalled(paths: ToolPaths): boolean {
  return Boolean(paths.hlaeExe && paths.hlaeDll && paths.ffmpegExe && paths.vrfExe);
}

type PickOptions = { directory?: boolean; filters?: Array<{ name: string; extensions: string[] }> };

/** Text field + browse button; empty means "use the auto-detected value" (shown as placeholder). */
function PathField({ label, value, placeholder, hint, note, description, action, sourceAction, checked, status, source, detectedPath, onChange, pick }: {
  label: string; value: string; placeholder?: string; hint?: string; note?: ReactNode; checked?: boolean; detectedPath?: string | null;
  status?: string; description?: string; action?: ReactNode; sourceAction?: ReactNode; source?: { repo: string; url: string; site?: 'official' | 'steam' }; onChange: (v: string) => void; pick: PickOptions;
}) {
  const { t } = useTranslation();
  const notify = useNotify();

  const browse = async () => {
    try {
      const defaultPath = await api.browseDirectory(value.trim() || placeholder || null);
      const picked = await open({ multiple: false, directory: pick.directory ?? false, filters: pick.filters, defaultPath });
      if (typeof picked === 'string') onChange(picked);
    } catch (error) {
      notify(errorText(error));
    }
  };
  return (

    <Box className={source ? 'path-field tool-field' : 'path-field'}>
      <Flex justify="between" align="center" mb="1" gap="2">
        <Text size="2" weight="medium">{label}</Text>
        {(checked !== undefined || status) && <Text size="1" color={checked ? 'green' : checked === false ? 'red' : 'gray'}>
          <i aria-hidden="true" className={'bi app-icon ' + (checked === undefined ? 'bi-hourglass-split' : checked ? 'bi-check-circle' : 'bi-x-circle')} />
          {' '}{status ?? t(checked ? 'settings.ready' : 'settings.notReady')}
        </Text>}
      </Flex>
      {(description || action) && <Flex justify="between" align="center" mb="2" gap="2">
        {description && <Text size="1" color="gray" style={{ flex: 1, minWidth: 0 }}>{description}</Text>}
        {action && <Flex align="center" gap="2" style={{ flexShrink: 0, marginLeft: 'auto' }}>{action}</Flex>}
      </Flex>}
      <Flex align="center" gap="2">
        <TextField.Root size="2" aria-label={label} value={value} placeholder={placeholder ?? t('settings.notDetected')} onChange={(e) => onChange(e.target.value)} className="mono path-input" style={{ flex: 1, minWidth: 0 }}>
        {value && (
          <TextField.Slot side="right">
          <Tooltip disableHoverableContent delayDuration={400} style={{ pointerEvents: 'none' }} content={t('settings.clear')}>
            <IconButton size="1" variant="ghost" color="gray" className="path-clear" onClick={() => onChange('')} aria-label={t('settings.clear')}>
              <i aria-hidden="true" className="bi bi-x-lg app-icon" />
            </IconButton>
          </Tooltip>
          </TextField.Slot>
        )}
        </TextField.Root>
        <Tooltip disableHoverableContent delayDuration={400} style={{ pointerEvents: 'none' }} content={t('settings.browse')}><IconButton size="2" variant="outline" color="gray" aria-label={t('settings.browse')} onClick={() => void browse()}>
          <i aria-hidden="true" className="bi bi-three-dots app-icon" />
        </IconButton></Tooltip>
        <Tooltip disableHoverableContent delayDuration={400} style={{ pointerEvents: 'none' }} content={t('common.openInExplorer')}>
          <IconButton size="2" variant="outline" color="gray" aria-label={t('common.openInExplorer')} disabled={!detectedPath} onClick={() => { if (detectedPath) void api.open(pick.directory ? detectedPath : detectedPath.replace(/[^\\/]+$/, '')).catch(error => notify(errorText(error))); }}>
            <i aria-hidden="true" className="bi bi-folder2-open app-icon" />
          </IconButton>
        </Tooltip>
        {sourceAction ?? (source && <Tooltip disableHoverableContent delayDuration={400} style={{ pointerEvents: 'none' }} content={source.site === 'steam' ? t('settings.steamStoreSource', { name: source.repo }) : source.site === 'official' ? t('settings.officialSource', { name: source.repo }) : t('settings.source', { repo: source.repo })}>
          <IconButton size="2" variant="outline" color="gray" aria-label={t('settings.sourceLabel') + ': ' + label} onClick={() => void api.openUrl(source.url)}>
            <i aria-hidden="true" className="bi bi-box-arrow-up-right app-icon" />
          </IconButton>
        </Tooltip>)}
      </Flex>
      {hint && (
        <Text as="div" size="1" color="gray" mt="1">
          {hint}
        </Text>
      )}
      {note}
    </Box>

  );
}

/** Storage categories share the data folder selected above them. */
function StorageRow({ label, what, path, bytes, confirm, onClear }: { label: string; what: string; path: string; bytes: number | undefined; confirm: string; onClear: () => Promise<void> }) {
  const { t } = useTranslation();
  const clearingRef = useRef(false);
  const [clearing, setClearing] = useState(false);
  const clear = async () => {
    if (clearingRef.current) return;
    clearingRef.current = true;
    setClearing(true);
    try {
      await onClear();
    } finally {
      clearingRef.current = false;
      setClearing(false);
    }
  };
  return (
    <>
      <Text size="2" weight="medium" style={{ minWidth: 0 }}>{label}</Text>
      <Text size="2" color="gray" align="right" style={{ fontVariantNumeric: 'tabular-nums', whiteSpace: 'nowrap' }}>{bytes === undefined ? t('settings.calculating') : mb(bytes)}</Text>
      <IconButton size="2" variant="outline" color="gray" aria-label={t('common.openInExplorer')} onClick={() => void api.open(path)}>
        <i aria-hidden="true" className="bi bi-folder2-open app-icon"  />
      </IconButton>
      <ConfirmDialog title={t('settings.emptyTitle', { what })} description={confirm} confirmLabel={t('settings.empty')}
        onConfirm={() => void clear()} trigger={<Button size="2" variant="outline" color="red" aria-busy={clearing} disabled={clearing || bytes === undefined || bytes === 0}>{t('settings.empty')}</Button>} />
    </>
  );
}

/** Where a downloaded tool comes from; shown as a link icon after its name. */
const SOURCES = {
  steam: { site: 'official' as const, repo: 'Steam', url: 'https://store.steampowered.com/about/' },
  cs2: { site: 'steam' as const, repo: 'Counter-Strike 2', url: 'https://store.steampowered.com/app/730/' },
  hlae: { repo: 'advancedfx/advancedfx', url: 'https://github.com/advancedfx/advancedfx/releases' },
  ffmpeg: { repo: 'BtbN/FFmpeg-Builds (win64 gpl-shared)', url: 'https://github.com/BtbN/FFmpeg-Builds/releases' },
  vrf: { repo: 'ValveResourceFormat/ValveResourceFormat', url: 'https://github.com/ValveResourceFormat/ValveResourceFormat/releases' },
};

export function SettingsView({ onChanged, toolsRequest = 0, toolsTarget = 'render', group = 'all', active=true, storageKey='', onGuardChange }: { onChanged: () => Promise<void>; toolsRequest?: number; toolsTarget?: 'render' | 'replay'; group?: 'all'|'recording'|'appearance'|'tools'|'storage'; active?:boolean; storageKey?:string; onGuardChange?: (guard:DraftGuard|undefined)=>void }) {
  const { t } = useTranslation();
  const [data, setData] = useState<SettingsResponse>();
  const [storage, setStorage] = useState<StorageBytes>();
  const [storageError, setStorageError] = useState<string>();
  const [storageRevision, setStorageRevision] = useState(0);
  const storageRequest = useRef<Promise<StorageBytes> | undefined>(undefined);
  const [checking, setChecking] = useState(false);
  const [diagnostics, setDiagnostics] = useState<string>();
  const activeDataDir = data?.dataDir;
  useEffect(() => {
    if (!activeDataDir || !active || (group!=='all'&&group!=='storage')) return;
    let disposed = false;
    setStorage(undefined);
    setStorageError(undefined);
    void (async()=>{
      // A category change or completed export may require fresh counts, but
      // never starts another recursive scan while a previous one is in flight.
      while(storageRequest.current){try{await storageRequest.current;}catch{/* Current read reports its own error. */}if(disposed)return;}
      if(disposed)return;
      const request=api.storageBytes();storageRequest.current=request;
      try{const value=await request;if(!disposed)setStorage(value);}
      catch(error){if(!disposed)setStorageError(errorText(error));}
      finally{if(storageRequest.current===request)storageRequest.current=undefined;}
    })();
    return () => { disposed = true; };
  }, [activeDataDir, storageRevision, active, group, storageKey]);
  const toolsRef = useRef<HTMLDivElement>(null);
  const toolGuide = useRef<ReturnType<typeof driver> | null>(null);
  const dismissedRequest = useRef(0);
  const loaded = data !== undefined;
  const guidePhase = data && JSON.stringify([
    data.doctor.paths.steamDir, data.doctor.paths.cs2Exe, data.doctor.paths.hlaeExe,
    data.doctor.paths.hlaeDll, data.doctor.paths.ffmpegExe, data.doctor.paths.vrfExe,
    ...(['hlae', 'ffmpeg', 'vrf'] as const).map(tool => [Boolean(data.setup[tool]?.running), Boolean(data.setup[tool]?.progress)]),
  ]);
  useEffect(() => {
    if (!toolsRequest || toolsRequest === dismissedRequest.current || !loaded || !toolsRef.current) return;
    const buttons = [...toolsRef.current.querySelectorAll<HTMLButtonElement>('[data-guide-missing="true"]')];
    const first = buttons[0];
    if (!first || buttons.some(button => button.disabled)) return;
    const downloading = buttons.every(button => button.dataset.guideDownloading === 'true');
    let target: HTMLElement = first.closest<HTMLElement>('.tool-field') ?? first;
    while (target.parentElement && !buttons.every(button => target.contains(button))) target = target.parentElement;
    target.scrollIntoView({ block: 'center', behavior: 'instant' });
    let refreshing = false;
    const guide = driver({
      animate: false,
      overlayOpacity: 0.65,
      popoverClass: 'tools-guide',
      onDestroyed: () => { if (!refreshing) dismissedRequest.current = toolsRequest; buttons.forEach(button => button.classList.remove('tools-highlight')); },
      onPopoverRender: popover => popover.closeButton.setAttribute('aria-label', t('common.close')),
    });
    toolGuide.current = guide;
    guide.highlight({
      element: target,
      popover: { showButtons: ['close'], title: buttons.length === 1 ? first.getAttribute('aria-label') ?? '' : (downloading ? 'Log' : t('settings.downloadTool')) + ': ' + buttons.map(button => button.dataset.toolLabel).join(', '), description: t(downloading ? 'settings.waitForDownload' : toolsTarget === 'render' ? 'settings.renderToolsReason' : 'settings.replayToolReason'), side: 'left', align: 'center' },
    });
    buttons.forEach(button => button.classList.add('tools-highlight'));
    const frame = requestAnimationFrame(() => guide.refresh());
    return () => { refreshing = true; cancelAnimationFrame(frame); guide.destroy(); toolGuide.current = null; };
  }, [toolsRequest, toolsTarget, loaded, guidePhase, t]);
  const [form, setForm] = useState<Settings>({ language: null, cs2Dir: null, steamDir: null, replayFolders: [], scanGameReplays: true, hlaeExe: null, ffmpegExe: null, vrfExe: null });
  const [dataDirOverride, setDataDirOverride] = useState('');
  const [saving, setSaving] = useState(false);
  const [pendingSave, setPendingSave] = useState<{ settings: Settings; original: Settings; target: string }>();
  const downloadSelections = useRef<Partial<Record<'hlae' | 'ffmpeg' | 'vrf', string | null>>>({});
  const [message, setMessage] = useState<{ ok: boolean; text: string }>();
  const [loadAttempt, setLoadAttempt] = useState(0);
  const [logTool, setLogTool] = useState<'hlae' | 'ffmpeg' | 'vrf'>();
  const [updates, setUpdates] = useState<Partial<Record<'hlae' | 'ffmpeg' | 'vrf', ToolUpdate | { error: string }>>>({});
  const [checkingUpdates, setCheckingUpdates] = useState(false);

  const load = useCallback(async () => {
    const r = await api.settings();
    setData(r);
    return r;
  }, []);
  useEffect(() => {
    let disposed = false;
    api.settings()
      .then((r) => {
        if (disposed) return;
        setData(r);
        setForm(r.settings); setDataDirOverride(r.dataDirOverride ?? '');
      })
      .catch((e) => { if (!disposed) setMessage({ ok: false, text: errorText(e) }); });
    let unlisten: (() => void) | undefined;
    void api
      .onEvent((ev) => {
        if (disposed) return;
        if (ev.type === 'setup-progress') setData(current => current && ({ ...current, setup: { ...current.setup, [ev.tool]: { log: [], ...current.setup[ev.tool], running: true, progress: ev.progress } } }));
        if (ev.type === 'setup-finished') {
          setUpdates(current => ({ ...current, [ev.tool]: undefined }));
          void load().then(r => {
            if (!ev.ok && !ev.installed) return;
            const field = ({ hlae: 'hlaeExe', ffmpeg: 'ffmpegExe', vrf: 'vrfExe' } as const)[ev.tool];
            setForm(current => current[field] === downloadSelections.current[ev.tool] ? ({ ...current, [field]: r.settings[field] }) : current);
          }).catch(e => setMessage({ ok: false, text: errorText(e) }));
          setMessage(ev.cancelled ? { ok: true, text: i18n.t('renders.status.cancelled') } : ev.ok ? { ok: true, text: i18n.t('settings.downloadDone') } : { ok: false, text: i18n.t('settings.downloadFailed', { error: ev.error ?? i18n.t('settings.unknownError') }) });
        }
      })
      .then((stop) => { if (disposed) stop(); else unlisten = stop; })
      .catch(e => { if (!disposed) setMessage({ ok: false, text: errorText(e) }); });
    return () => { disposed = true; unlisten?.(); };
  }, [load, loadAttempt]);

  const save = async () => {
    if (!data || saving || pendingSave) return false;
    setSaving(true);
    setMessage(undefined);
    try {
      const preview = await api.previewSettings(form, data.settings, dataDirOverride || null);
      if (preview.restartRequired) {
        setPendingSave({ settings: structuredClone(form), original: structuredClone(data.settings), target: preview.target });
        return false;
      }
      const r = await api.saveSettings(form, preview.target, data.settings);
      if (!r) return false;
      setData(r);
      setForm(r.settings);
      setDataDirOverride(r.dataDirOverride ?? '');
      applyLanguage(r.settings.language);
      // t() is still bound to the old language here; say it in the one just saved
      setMessage({ ok: true, text: t('settings.saved', { lng: r.settings.language ?? detectLanguage() }) });
      await onChanged();
      if ((['steamDir', 'cs2Dir', 'hlaeExe', 'ffmpegExe', 'vrfExe'] as const).some(key => r.settings[key] !== data.settings[key])) {
        setChecking(true);
        setData(await api.checkTools());
      }
      return true;
    } catch (e) {
      setMessage({ ok: false, text: errorText(e) });
      return false;
    } finally {
      setSaving(false);
      setChecking(false);
    }
  };
  /** Re-run detection with what is saved (paths typed but not saved are not checked). */
  const check = async () => {
    setMessage(undefined);
    setChecking(true);
    try {
      const r = await api.checkTools();
      setData(r);
      setMessage(!toolsInstalled(r.doctor.paths) ? { ok: false, text: t('settings.toolsIncompleteToast') }
        : Object.values(r.toolChecks ?? {}).some(check => !check.ok) ? { ok: false, text: t('settings.startupFailedHint') }
        : r.doctor.ok ? { ok: true, text: t('settings.envReady') }
        : { ok: false, text: t('settings.envNotReady', { problems: r.doctor.problems.map(translateProblem).join('; ') }) });
      await onChanged();
    } catch (e) {
      setMessage({ ok: false, text: errorText(e) });
    } finally {
      setChecking(false);
    }
  };
  /** Only on demand: lists the installed and online versions; updating is the user's decision. */
  const checkUpdates = async () => {
    const tools = (['hlae', 'ffmpeg', 'vrf'] as const).filter(tool => ready[tool]);
    setCheckingUpdates(true);
    setUpdates({});
    const results = await Promise.all(tools.map(tool => api.checkToolUpdate(tool).catch(error => ({ error: errorText(error) }))));
    setUpdates(Object.fromEntries(tools.map((tool, index) => [tool, results[index]])));
    setCheckingUpdates(false);
  };
  const updateNote = (tool: 'hlae' | 'ffmpeg' | 'vrf') => {
    const result = updates[tool];
    if (!result) return checkingUpdates && ready[tool] ? <Flex justify="center" align="center" mt="2" style={{ minHeight: 'calc(2 * var(--line-height-1))' }}><Spinner size="1" /></Flex> : undefined;
    if ('error' in result) return <Text as="div" size="1" color="red" mt="1">{t('settings.updateCheckFailed', { error: result.error })}</Text>;
    return <Text as="div" size="1" color="gray" mt="1">
      <div>{t('settings.installedVersion', { version: result.installed ?? t('settings.versionUnknown') })}</div>
      <div>{t('settings.onlineVersion', { version: result.latest })}</div>
    </Text>;
  };
  const confirmDirectoryChange = async () => {
    if (!pendingSave || saving) return;
    const submission = pendingSave;
    setPendingSave(undefined);
    setSaving(true);
    try {
      const r = await api.saveSettings(submission.settings, submission.target, submission.original, true);
      setData(r);
      setForm(r.settings);
      setDataDirOverride(r.dataDirOverride ?? '');
      setMessage({ ok: true, text: t('settings.restartRequired') });
    } catch (error) {
      setMessage({ ok: false, text: errorText(error) });
    } finally { setSaving(false); }
  };
  const clear = (what: string, fn: () => Promise<number>) => async () => {
    try {
      const freed = await fn();
      setStorageRevision(current => current + 1);
      setMessage({ ok: true, text: t('settings.cleared', { what, size: mb(freed) }) });
      await load();
      await onChanged();
    } catch (e) {
      setMessage({ ok: false, text: errorText(e) });
    }
  };
  // Always a fresh download: the same button re-installs after a CS2 update breaks HLAE.
  const runSetup = async (tool: 'hlae' | 'ffmpeg' | 'vrf') => {
    setMessage(undefined);
    setData(current => current && ({ ...current, setup: { ...current.setup, [tool]: { running: true, progress: null, log: [] } } }));
    try {
      const field = ({ hlae: 'hlaeExe', ffmpeg: 'ffmpegExe', vrf: 'vrfExe' } as const)[tool];
      downloadSelections.current[tool] = form[field];
      await api.runSetup(tool, true, toolDirectory(form[field]) || null);
      await load();
    } catch (e) {
      setData(current => current && ({ ...current, setup: { ...current.setup, [tool]: { running: false, progress: null, log: [errorText(e)] } } }));
      setMessage({ ok: false, text: errorText(e) });
    }
  };

  const guardDirty = !!data && (JSON.stringify(form) !== JSON.stringify(data.settings) || dataDirOverride !== (data.dataDirOverride ?? ''));
  useEffect(() => {
    onGuardChange?.({dirty:guardDirty,save,discard:()=>{if(data){setForm(data.settings);setDataDirOverride(data.dataDirOverride??'');}setPendingSave(undefined);setMessage(undefined);}});
    return()=>onGuardChange?.(undefined);
  },[guardDirty,form,data,dataDirOverride,saving,pendingSave,onGuardChange]);
  if (!data) return <Flex direction="column" gap="3" align="start"><Text color={message?.ok === false ? 'red' : 'gray'} role={message?.ok === false ? 'alert' : 'status'}>{message?.text ?? t('settings.loading')}</Text>{message?.ok === false && <Button variant="soft" onClick={() => { setMessage(undefined); setLoadAttempt(attempt => attempt + 1); }}>{t('studio.retryLoadingSettings', { defaultValue: 'Retry loading settings' })}</Button>}</Flex>;
  const d = data.doctor;
  const toolsReady = toolsInstalled(d.paths);
  const ready = { steam: Boolean(d.paths.steamDir), cs2: Boolean(d.paths.cs2Exe), hlae: Boolean(d.paths.hlaeExe && d.paths.hlaeDll), ffmpeg: Boolean(d.paths.ffmpegExe), vrf: Boolean(d.paths.vrfExe) };
  const toolReadiness = (tool: 'hlae' | 'ffmpeg' | 'vrf') => {
    const field = ({hlae: 'hlaeExe', ffmpeg: 'ffmpegExe', vrf: 'vrfExe'} as const)[tool];
    if ((form[field] ?? '') !== (data.settings[field] ?? '')) return {};
    if (!ready[tool]) return {checked: false};
    if (checking) return {status: t('settings.verifying')};
    const result = data.toolChecks?.[tool];
    if (!result) return {status: t('settings.unverified')};
    return {checked: result.ok, status: result.ok ? t('settings.ready') : t('settings.startupFailed')};
  };
  const guidedTools = (toolsTarget === 'render' ? ['steam', 'cs2', 'hlae', 'ffmpeg'] as const : ['vrf'] as const).filter(tool => !ready[tool]);
  const toolButton = (tool: 'steam' | 'cs2' | 'hlae' | 'ffmpeg' | 'vrf', label: string) => {
    const setup = tool === 'steam' || tool === 'cs2' ? undefined : data.setup[tool];
    const openLog = () => { toolGuide.current?.destroy(); if (tool !== 'steam' && tool !== 'cs2') setLogTool(tool); };
    const logGuide = { 'data-guide-missing': guidedTools.includes(tool) && Boolean(setup?.running), 'data-guide-downloading': true, 'data-tool-label': label };
    return (<>
    {setup && (setup.running || setup.log.length > 0) && (
      <Tooltip disableHoverableContent delayDuration={400} style={{ pointerEvents: 'none' }} content="Log">
        {setup.running && setup.progress ? (
          <Button {...logGuide} size="2" variant="outline" color="gray" aria-label={t('settings.setupLogTitle') + ': ' + label} onClick={openLog}>
            {setup.progress.trim()}
          </Button>
        ) : (
          <IconButton {...logGuide} size="2" variant="outline" color="gray" aria-label={t('settings.setupLogTitle') + ': ' + label} onClick={openLog}>
            <i aria-hidden="true" className="bi bi-file-text app-icon" />
          </IconButton>
        )}
      </Tooltip>
    )}
    <Tooltip disableHoverableContent delayDuration={400} style={{ pointerEvents: 'none' }} content={setup?.running ? t('common.cancel') : tool === 'steam' ? t('settings.officialSource', { name: label }) : tool === 'cs2' ? t('settings.steamStoreSource', { name: label }) : t('settings.downloadTool') + ': ' + label}>
      <IconButton data-guide-missing={guidedTools.includes(tool) && !setup?.running} data-tool-label={label}
        size="2" variant="outline" color="gray" aria-label={t(setup?.running ? 'common.cancel' : tool === 'steam' || tool === 'cs2' ? 'settings.sourceLabel' : 'settings.downloadTool') + ': ' + label}
        disabled={setup?.stopping} aria-busy={setup?.stopping} onClick={() => {
          toolGuide.current?.destroy();
          if (tool === 'steam' || tool === 'cs2') {
            void api.openUrl(SOURCES[tool].url).catch(error => setMessage({ ok: false, text: errorText(error) }));
          } else if (setup?.running) {
            setData(current => current && ({ ...current, setup: { ...current.setup, [tool]: { ...setup, stopping: true } } }));
            void api.cancelSetup(tool).catch(error => { void load(); setMessage({ ok: false, text: errorText(error) }); });
          } else {
            void runSetup(tool);
          }
        }}>
        <i aria-hidden="true" className={setup?.running ? 'bi bi-stop-fill app-icon' : tool === 'steam' || tool === 'cs2' ? 'bi bi-box-arrow-up-right app-icon' : 'bi bi-download app-icon'} />
      </IconButton>
    </Tooltip>
    </>);
  };

  const set = (patch: Partial<Settings>) => {
    setForm({ ...form, ...patch });
    setMessage(undefined);
  };
  const dirty = JSON.stringify(form) !== JSON.stringify(data.settings) || dataDirOverride !== (data.dataDirOverride ?? '');
  const folders = form.replayFolders.filter(Boolean);
  const gameReplays = d.paths.cs2Dir ? d.paths.cs2Dir + '/game/csgo/replays' : (form.cs2Dir ? `${form.cs2Dir}\\game\\csgo\\replays` : undefined);

  return (
    <fieldset disabled={saving} style={{border:0,padding:0,margin:0,minWidth:0}}><Flex direction="column" gap="4" className="settings-page">
      <ConfirmDialog open={Boolean(pendingSave)} onOpenChange={open => { if (!open) setPendingSave(undefined); }}
        title={t('settings.changeDataTitle')} description={t('settings.changeDataConfirm')}
        confirmLabel={t('common.save')} onConfirm={() => void confirmDirectoryChange()} />
      <Flex justify="between" align="center" gap="3" wrap="wrap">
        <Box>
          <Heading data-text-role="subtitle" size="6">{group==='all'?t('settings.title'):group==='recording'?'Recording defaults':group==='appearance'?'Language and display':group==='storage'?'Storage and demo sources':'Tools and installation'}</Heading>
        </Box>
        <Flex gap="2" align="center">
          <Button onClick={() => void save()} disabled={saving || !dirty}>
            {saving ? t('settings.saving') : t('common.save')}
          </Button>
        </Flex>
      </Flex>

      {dirty && (
        <Callout.Root color="amber" size="1">
          <Callout.Icon>
            <i aria-hidden="true" className="bi bi-info-circle app-icon"  />
          </Callout.Icon>
          {/* in the language being picked, so the user can read it before saving */}
          <Callout.Text>{t('settings.unsaved', { lng: form.language ?? detectLanguage() })}</Callout.Text>
        </Callout.Root>
      )}

      {message && <Toast message={message.text} color={message.ok ? 'green' : 'red'} duration={message.ok ? 3000 : 0} onDismiss={() => setMessage(undefined)} />}

      <Grid className="settings-panels" columns="repeat(auto-fit, minmax(min(100%, 480px), 1fr))" gap="4" align="start">
        <Card hidden={group!=='all'&&group!=='recording'} className="recording-defaults-card">
          <Heading data-text-role="subtitle">Your default export quality</Heading>
          <Text as="p" color="gray" mt="2" mb="4">New manual and automatic exports use these settings. Existing jobs and AI preview recordings keep their own settings.</Text>
          <Grid columns={{initial:'1',sm:'2'}} gap="4">
            <Box><Text as="label">Resolution</Text><Select.Root value={String((form.renderDefaults??DEFAULT_RENDER_OPTIONS).height)} onValueChange={value=>{const height=Number(value);set({renderDefaults:{...(form.renderDefaults??DEFAULT_RENDER_OPTIONS),height,width:({720:1280,1080:1920,1440:2560,2160:3840} as Record<number,number>)[height]!}});}}><Select.Trigger aria-label="Default recording resolution"/><Select.Content>{[720,1080,1440,2160].map(height=><Select.Item key={height} value={String(height)}>{height===2160?'4K · 3840 × 2160':height+'p'}</Select.Item>)}</Select.Content></Select.Root></Box>
            <Box><Text as="label">Frame rate</Text><Select.Root value={String((form.renderDefaults??DEFAULT_RENDER_OPTIONS).fps)} onValueChange={value=>set({renderDefaults:{...(form.renderDefaults??DEFAULT_RENDER_OPTIONS),fps:Number(value)}})}><Select.Trigger aria-label="Default recording frame rate"/><Select.Content>{[30,60,120].map(fps=><Select.Item key={fps} value={String(fps)}>{fps} FPS</Select.Item>)}</Select.Content></Select.Root></Box>
            <Box><Text as="label">Encoder</Text><Select.Root value={(form.renderDefaults??DEFAULT_RENDER_OPTIONS).codec} onValueChange={codec=>set({renderDefaults:{...(form.renderDefaults??DEFAULT_RENDER_OPTIONS),codec,crf:RENDER_QUALITY[codec]??18}})}><Select.Trigger aria-label="Default recording encoder"/><Select.Content>{[['h264_amf','H.264 · AMD'],['hevc_amf','H.265 · AMD'],['libx264','H.264 · CPU'],['libx265','H.265 · CPU'],['h264_nvenc','H.264 · NVIDIA'],['hevc_nvenc','H.265 · NVIDIA']].map(([codec,label])=><Select.Item key={codec} value={codec!}>{label}</Select.Item>)}</Select.Content></Select.Root></Box>
            <Box><Text as="label" htmlFor="default-recording-format">{t('studio.fileFormat',{defaultValue:'File format'})}</Text><Select.Root value={(form.renderDefaults??DEFAULT_RENDER_OPTIONS).container} disabled={saving} onValueChange={container=>set({renderDefaults:{...(form.renderDefaults??DEFAULT_RENDER_OPTIONS),container}})}><Select.Trigger id="default-recording-format" aria-label={t('studio.fileFormat',{defaultValue:'File format'})}/><Select.Content><Select.Item value="mp4">MP4</Select.Item><Select.Item value="mkv">MKV</Select.Item></Select.Content></Select.Root></Box>
            <Box><Text as="label">File size</Text><Select.Root value={String((form.renderDefaults??DEFAULT_RENDER_OPTIONS).maxSizeMb)} onValueChange={value=>set({renderDefaults:{...(form.renderDefaults??DEFAULT_RENDER_OPTIONS),maxSizeMb:value==='null'?null:Number(value)}})}><Select.Trigger aria-label="Default recording size limit"/><Select.Content><Select.Item value="null">Unlimited</Select.Item>{[10,20,50].map(size=><Select.Item key={size} value={String(size)}>{size} MB</Select.Item>)}</Select.Content></Select.Root></Box>
          </Grid>
          <Flex gap="4" wrap="wrap" mt="4">{(['hud','chat','voice'] as const).map(key=><Text as="label" key={key}><Switch checked={(form.renderDefaults??DEFAULT_RENDER_OPTIONS)[key]} onCheckedChange={value=>set({renderDefaults:{...(form.renderDefaults??DEFAULT_RENDER_OPTIONS),[key]:value}})}/>{' '+({hud:'Show HUD',chat:'Show text chat',voice:'Voice chat, when recorded'})[key]}</Text>)}</Flex>
          <Button variant="soft" mt="4" onClick={()=>set({renderDefaults:structuredClone(DEFAULT_RENDER_OPTIONS)})}>Reset to native 4K60 defaults</Button>
        </Card>
        {/* ---- left column: game & demos, output ---- */}
        <Flex direction="column" gap="4">
          <Card hidden={group!=='all'&&group!=='appearance'}>
            <Flex justify="between" align="center" gap="3">
              <Text size="2" weight="medium">
                {t('settings.language')}
              </Text>
              <Select.Root value={form.language ?? 'auto'} onValueChange={(v) => set({ language: v === 'auto' ? null : v })}>
                <Select.Trigger className="bounded-select" aria-label={t('settings.language')} />
                <Select.Content>
                  <Select.Item value="auto">{t('settings.languageAuto')}</Select.Item>
                  {LANGUAGES.map((l) => (
                    <Select.Item key={l} value={l}>
                      {LANGUAGE_NAMES[l]}
                    </Select.Item>
                  ))}
                </Select.Content>
              </Select.Root>
            </Flex>
          </Card>


          <Card hidden={group!=='all'&&group!=='storage'}>
            <Heading data-text-role="subtitle" size="3" mb="3">{t('settings.demoSources')}</Heading>
            <Flex direction="column" gap="4">
              <Flex justify="between" align="center" gap="3">
                <Box style={{ minWidth: 0 }}>
                  <Text as="div" size="2" weight="medium">
                    {t('settings.scanGameReplays')}
                  </Text>
                  <Text as="div" size="1" color="gray" className="mono selectable" style={{ overflowWrap: 'anywhere', marginTop: 6 }}>
                    {gameReplays ?? t('settings.noCs2')}
                  </Text>
                </Box>
                <Switch aria-label={t('settings.scanGameReplays')} checked={form.scanGameReplays} onCheckedChange={(v) => set({ scanGameReplays: v })} />
              </Flex>
              <Box>
                <Flex justify="between" align="center" mb="1">
                  <Text size="2" weight="medium">
                    {t('settings.extraFolders')}
                  </Text>
                  <Button
                    size="2"
                    variant="outline" color="gray"
                    onClick={() =>
                      void open({ directory: true, multiple: false }).then((p) => {
                        if (typeof p === 'string' && !folders.includes(p)) set({ replayFolders: [...folders, p] });
                      })
                    }
                  >
                    <i aria-hidden="true" className="bi bi-plus-lg app-icon"  /> {t('settings.addFolder')}
                  </Button>
                </Flex>
                {folders.length === 0 ? (
                  <Text size="1" color="gray">
                    {t('settings.noExtraFolders')}
                  </Text>
                ) : (
                  <Flex direction="column" gap="1">
                    {folders.map((f) => (
                      <Flex key={f} align="center" gap="2" className="folder-row">
                        <Text size="2" className="mono selectable" style={{ flex: 1, minWidth: 0, overflowWrap: 'anywhere' }}>
                          {f}
                        </Text>
                        <IconButton size="2" variant="outline" color="gray" aria-label={t('settings.removeFolder')} onClick={() => set({ replayFolders: folders.filter((x) => x !== f) })}>
                          <i aria-hidden="true" className="bi bi-x-lg app-icon"  />
                        </IconButton>
                      </Flex>
                    ))}
                  </Flex>
                )}
              </Box>
            </Flex>
          </Card>

          <Card hidden={group!=='all'&&group!=='storage'}>
            <Flex justify="between" align="center" wrap="wrap" gap="3" mb="3"><Heading data-text-role="subtitle" size="3">{t('settings.storageSection')}</Heading><Button variant="soft" disabled={!storage&&!storageError} onClick={()=>setStorageRevision(revision=>revision+1)}>{t('studio.refreshStorage',{defaultValue:'Refresh storage'})}</Button></Flex>
            <Flex direction="column" gap="3">
              <PathField label={t('settings.dataDir')} detectedPath={data.dataDir} value={dataDirOverride} placeholder={data.defaultDataDir} hint={t('settings.dataDirHint')} onChange={(value) => { setDataDirOverride(value); setMessage(undefined); }} pick={{ directory: true }} />
              {data.restartRequired && (
                <Callout.Root color="amber" size="1">
                  <Callout.Text>{t('settings.restartRequired')}</Callout.Text>
                </Callout.Root>
              )}
              <Grid columns="minmax(0, 1fr) max-content max-content max-content" gapX="3" gapY="3" align="center">
                {storageError && <Text color="red" style={{ gridColumn: '1 / -1' }}>{storageError}</Text>}
                <StorageRow label={t('settings.parsedDir')} what={t('settings.clearParsedWhat')} path={`${data.dataDir}\\parsed`} bytes={storage?.parsedBytes} confirm={t('settings.clearParsedConfirm')} onClear={clear(t('settings.clearParsedWhat'), api.clearAllAnalysis)} />
                <StorageRow label={t('settings.anomalyDir')} what={t('settings.anomalyDir')} path={`${data.dataDir}\\analysis`} bytes={storage?.anomalyBytes} confirm={t('settings.clearAnomalyConfirm')} onClear={clear(t('settings.anomalyDir'), api.clearAnomalyData)} />
                <StorageRow label={t('settings.clipsDir')} what={t('settings.clearClipsWhat')} path={`${data.dataDir}\\clips`} bytes={storage?.clipsBytes} confirm={t('settings.clearClipsConfirm')} onClear={clear(t('settings.clearClipsWhat'), api.clearAllClips)} />
                <StorageRow label={t('settings.radarDir')} what={t('settings.clearRadarWhat')} path={`${data.dataDir}\\radar`} bytes={storage?.radarBytes} confirm={t('settings.clearRadarConfirm')} onClear={clear(t('settings.clearRadarWhat'), api.clearRadar)} />
              </Grid>
            </Flex>
          </Card>
        </Flex>

        {/* ---- right column: game environment & render tools ---- */}
        <Flex ref={toolsRef} direction="column" gap="4">
          <Card hidden={group!=='all'&&group!=='tools'}>
            <Heading data-text-role="subtitle" size="3" mb="3">
              {t('settings.gameSection')}
            </Heading>
            <Flex direction="column" gap="3">
              <PathField label={t('settings.steamDir')} description={t('settings.steamPurpose')} source={SOURCES.steam} sourceAction={toolButton('steam', 'Steam')} checked={(form.steamDir ?? '') === (data.settings.steamDir ?? '') ? ready.steam : undefined} value={form.steamDir ?? ''} detectedPath={(form.steamDir ?? '') === (data.settings.steamDir ?? '') ? d.paths.steamDir : null} placeholder={d.paths.steamDir} onChange={value => set({ steamDir: value || null })} pick={{ directory: true }} />
              <PathField description={t('settings.cs2Purpose')} source={SOURCES.cs2} sourceAction={toolButton('cs2', 'CS2')} checked={(form.cs2Dir ?? '') === (data.settings.cs2Dir ?? '') ? Boolean(d.paths.cs2Exe) : undefined} hint={d.paths.cs2PatchVersion ? 'CS2 v' + d.paths.cs2PatchVersion : undefined} label={t('settings.cs2Dir')} value={form.cs2Dir ?? ''} detectedPath={(form.cs2Dir ?? '') === (data.settings.cs2Dir ?? '') ? (d.paths.cs2Exe ? d.paths.cs2Dir : null) : null} placeholder={d.paths.cs2Dir} onChange={(v) => set({ cs2Dir: v || null })} pick={{ directory: true }} />
            </Flex>
          </Card>
          <Card hidden={group!=='all'&&group!=='tools'}>
            <Heading data-text-role="subtitle" size="3" mb="3">{t('settings.toolsSection')}</Heading>
            {!toolsReady && (
              <Callout.Root color="red" size="1" mb="3">
                <Callout.Icon>
                  <i aria-hidden="true" className="bi bi-exclamation-triangle app-icon"  />
                </Callout.Icon>
                <Callout.Text>
                  {t('settings.toolsIncomplete')}
                </Callout.Text>
              </Callout.Root>
            )}
            <Flex direction="column" gap="3">
              <Flex direction="column" gap="3">
                <PathField description={t('settings.hlaePurpose')} note={updateNote('hlae')} action={toolButton('hlae', 'HLAE')} source={SOURCES.hlae} {...toolReadiness('hlae')} label="HLAE" value={toolDirectory(form.hlaeExe)} detectedPath={(form.hlaeExe ?? '') === (data.settings.hlaeExe ?? '') ? toolDirectory(d.paths.hlaeExe) : null} placeholder={`${data.dataDir}/tools`} onChange={(v) => set({ hlaeExe: v || null })} pick={{ directory: true }} />
                <PathField description={t('settings.ffmpegPurpose')} note={updateNote('ffmpeg')} action={toolButton('ffmpeg', 'FFmpeg')} source={SOURCES.ffmpeg} {...toolReadiness('ffmpeg')} label="FFmpeg" value={toolDirectory(form.ffmpegExe)} detectedPath={(form.ffmpegExe ?? '') === (data.settings.ffmpegExe ?? '') ? toolDirectory(d.paths.ffmpegExe) : null} placeholder={`${data.dataDir}/tools`} onChange={(v) => set({ ffmpegExe: v || null })} pick={{ directory: true }} />
              </Flex>
              <PathField description={t('settings.vrfPurpose')} note={updateNote('vrf')} action={toolButton('vrf', 'Source 2 Viewer CLI')} source={SOURCES.vrf} {...toolReadiness('vrf')} label="Source 2 Viewer CLI" value={toolDirectory(form.vrfExe)} detectedPath={(form.vrfExe ?? '') === (data.settings.vrfExe ?? '') ? toolDirectory(d.paths.vrfExe) : null} placeholder={`${data.dataDir}/tools`} onChange={(v) => set({ vrfExe: v || null })} pick={{ directory: true }} />
              <Flex gap="2" wrap="wrap" align="center" justify="center">
                <Button size="2" variant="outline" color="gray" disabled={checking} onClick={() => void check()}>{checking ? t('settings.verifying') : t('settings.recheck')}</Button>
                <Button size="2" variant="outline" color="gray" disabled={checkingUpdates} onClick={() => void checkUpdates()}>{checkingUpdates ? <Spinner size="1" /> : t('settings.checkUpdates')}</Button>
                <Button size="2" variant="outline" color="gray" onClick={() => void api.toolDiagnostics().then(setDiagnostics).catch(error => setMessage({ok: false, text: errorText(error)}))}>{t('settings.diagnostics')}</Button>
              </Flex>
              {Object.values(data.toolChecks ?? {}).some(check => !check.ok && check.path) && <Text size="1" color="red">{t('settings.startupFailedHint')}</Text>}
            </Flex>

          </Card>
        </Flex>
      </Grid>

      <Dialog.Root open={diagnostics !== undefined} onOpenChange={open => { if (!open) setDiagnostics(undefined); }}>
        <Dialog.Content maxWidth="900px">
          <Dialog.Title>{t('settings.diagnostics')}</Dialog.Title>
          <Dialog.Description>{t('settings.diagnosticsHint')}</Dialog.Description>
          <pre style={{whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', maxHeight: '50vh', overflow: 'auto', userSelect: 'text'}}>{diagnostics}</pre>
          <Flex justify="end" gap="2">
            <Button variant="outline" onClick={() => void navigator.clipboard.writeText(diagnostics ?? '').then(() => setMessage({ok: true, text: t('settings.diagnosticsCopied')})).catch(error => setMessage({ok: false, text: errorText(error)}))}>{t('settings.copyDiagnostics')}</Button>
            <Dialog.Close><Button variant="outline">{t('common.close')}</Button></Dialog.Close>
          </Flex>
        </Dialog.Content>
      </Dialog.Root>
      <Dialog.Root open={logTool !== undefined} onOpenChange={open => { if (!open) setLogTool(undefined); }}>
        <Dialog.Content maxWidth="900px">
          <Dialog.Title>{t('settings.setupLogTitle')}</Dialog.Title>
          <Dialog.Description size="2" color="gray">
            {logTool && data.setup[logTool]?.running ? t('settings.setupRunning') : t('settings.setupFinished')}
          </Dialog.Description>
          <LogView lines={logTool ? data.setup[logTool]?.log ?? [] : []} empty={t('settings.setupLogEmpty')} />
          <Flex justify="end" mt="3">
            <Dialog.Close>
              <Button variant="outline">{t('common.close')}</Button>
            </Dialog.Close>
          </Flex>
        </Dialog.Content>
      </Dialog.Root>
    </Flex></fieldset>
  );
}
