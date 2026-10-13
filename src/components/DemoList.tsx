import { demoDate, compareDemoDates } from '../demoDate.ts';
import { useNotify, Toast } from './Notifications.tsx';
import { useAppTheme } from '../AppTheme.tsx';
import { Spinner } from './Spinner.tsx';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { format } from 'date-fns';
import { useTranslation } from 'react-i18next';
import { Badge, Box, Button, Flex, Heading, IconButton, Popover, Select, Text, TextField } from '@radix-ui/themes';
import { useVirtualizer } from '@tanstack/react-virtual';
import { open } from '@tauri-apps/plugin-dialog';
import { api, errorText, type DemoMeta, type RenderJob } from '../api.ts';
import { DateField, dayOf } from './DateField.tsx';

const STATUS_COLOR: Record<DemoMeta['status'], 'gray' | 'amber' | 'green' | 'red'> = { new: 'gray', queued: 'gray', validating: 'amber', parsing: 'amber', parsed: 'green', error: 'red' };
const savedFilters = { query: '', from: '', to: '', status: 'all' };

export function DemoList({
  demos,
  jobs,
  selectedId,
  active = true,
  toolbar,
  selectionRequest,
  onSelect,

  onChanged,
  onRefresh,
}: {
  demos: DemoMeta[];
  jobs: RenderJob[];
  selectedId?: string;
  active?: boolean;
  toolbar?: HTMLElement | null;
  selectionRequest: number;
  onSelect: (id: string) => void;

  onChanged: () => Promise<void>;
  onRefresh: () => Promise<boolean>;
}) {
  const { t } = useTranslation();
  const notify = useNotify();
  const { typography } = useAppTheme();
  const rowHeight = 40 + typography[2]! * 3 + typography[3]! * 2;
  const [busy, setBusy] = useState(false);
  const [refreshStatus, setRefreshStatus] = useState<'refreshing' | 'refreshed' | 'refreshFailed'>();
  const refreshRequest = useRef(0);
  useEffect(() => () => { refreshRequest.current++; }, []);
  const handleRefresh = async () => {
    if (refreshStatus === 'refreshing') return;
    const request = ++refreshRequest.current;
    setRefreshStatus('refreshing');
    try {
      const success = await onRefresh();
      if (request === refreshRequest.current) setRefreshStatus(success ? 'refreshed' : 'refreshFailed');
    } catch {
      if (request === refreshRequest.current) setRefreshStatus('refreshFailed');
    }
  };
  const [query, setQuery] = useState(savedFilters.query);
  const [from, setFrom] = useState(savedFilters.from);
  const [to, setTo] = useState(savedFilters.to);
  const [statusFilter, setStatusFilter] = useState(savedFilters.status);
  useEffect(() => { Object.assign(savedFilters, { query, from, to, status: statusFilter }); }, [query, from, to, statusFilter]);
  const scrollRef = useRef<HTMLDivElement>(null);

  const exportsByDemo = useMemo(() => {
    const summaries = new Map<string, { videos: number; queued: number; running: number; error: number }>();
    for (const job of jobs) {
      const summary = summaries.get(job.demoId) ?? { videos: 0, queued: 0, running: 0, error: 0 };
      summary.videos += job.outputs.length;
      if (job.status === 'queued' || job.status === 'running' || job.status === 'error') summary[job.status]++;
      summaries.set(job.demoId, summary);
    }
    return summaries;
  }, [jobs]);

  const visible = useMemo(() => {
    // Substring match, case-insensitive; several words must all appear (any order),
    // across file name, map name and player names.
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    return demos.filter((d) => {
      if (statusFilter !== 'all' && d.status !== statusFilter) return false;
      if (words.length) {
        const hay = [d.name, d.mapName ?? '', ...(d.summary?.players ?? [])].join(' ').toLowerCase();
        if (!words.every((w) => hay.includes(w))) return false;
      }
      const day = dayOf(new Date(demoDate(d)));
      if (from && day < from) return false;
      if (to && day > to) return false;
      return true;
    }).sort(compareDemoDates);
  }, [demos, query, from, to, statusFilter]);

  const getItemKey = useCallback((index: number) => visible[index]!.id, [visible]);
  const virtualizer = useVirtualizer({
    enabled: active,
    useAnimationFrameWithResizeObserver: true,
    count: visible.length,
    getScrollElement: () => scrollRef.current,
    getItemKey,
    estimateSize: () => rowHeight,
    overscan: 6,
  });

  /** Pick .dem files; the backend copies them into the replays folder and we parse them right away. */
  const addFiles = async () => {
    const picked = await open({ multiple: true, filters: [{ name: 'CS2 demo', extensions: ['dem'] }] });
    if (!picked) return;
    const paths = Array.isArray(picked) ? picked : [picked];
    setBusy(true);
    try {
      const metas = [];
      for (const p of paths) metas.push(await api.registerDemo(p));
      await onChanged();
      if (metas[0]) onSelect(metas[0].id);
    } catch (e) {
      notify(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  const handledSelection = useRef(0);
  const filtered = !!(query.trim() || from || to || statusFilter !== 'all');
  useEffect(() => {
    if (!active || !selectionRequest || handledSelection.current === selectionRequest) return;
    const index = visible.findIndex(d => d.id === selectedId);
    if (index >= 0) {
      virtualizer.scrollToIndex(index, { align: 'auto' });
      handledSelection.current = selectionRequest;
    } else if (demos.some(demo => demo.id === selectedId)) {
      // Only an explicit request for a hidden match bypasses the current filters.
      setQuery(''); setFrom(''); setTo(''); setStatusFilter('all');
    }
  }, [active, selectionRequest, selectedId, visible, demos, virtualizer]);

  useEffect(() => { virtualizer.measure(); }, [rowHeight, virtualizer]);

  const actions = <Flex align="center" gap="3" wrap="wrap" className="studio-match-library-toolbar">
        <Flex align="center" gap="2" wrap="wrap" style={{ minWidth: 0, maxWidth: '100%' }}>
            <Button size="2" variant="soft" onClick={() => void addFiles()} disabled={busy} title={t('demoList.addTooltip')}>
              <i aria-hidden="true" className="bi bi-plus-lg app-icon"  />
              {t('studio.importDemo', { defaultValue: 'Import demo' })}
            </Button>
            <Button size="2" variant="soft" onClick={() => void handleRefresh()} disabled={busy || refreshStatus === 'refreshing'} aria-busy={refreshStatus === 'refreshing'} title={t('demoList.rescanTooltip')}>
              {refreshStatus === 'refreshing' ? <Spinner size="1" /> : <i aria-hidden="true" className="bi bi-arrow-clockwise app-icon"  />}
              {t('studio.refresh', { defaultValue: 'Refresh' })}
            </Button>
        </Flex>
        <TextField.Root size="2" className="studio-match-search" style={{ flex: '1 1 240px' }} aria-label={t('studio.searchMatches', { defaultValue: 'Search matches' })} placeholder={t('demoList.searchPlaceholder')} value={query} onChange={(e) => setQuery(e.target.value)}>
          <TextField.Slot>
            <i aria-hidden="true" className="bi bi-search app-icon"  />
          </TextField.Slot>
          {query && (
            <TextField.Slot side="right">
              <IconButton size="2" variant="ghost" color="gray" onClick={() => setQuery('')} aria-label={t('demoList.clearSearch')}>
                <i aria-hidden="true" className="bi bi-x-lg app-icon"  />
              </IconButton>
            </TextField.Slot>
          )}
        </TextField.Root>
        <Popover.Root>
          <Popover.Trigger><Button variant="soft" className="filter-trigger" data-filtered={filtered}><i aria-hidden="true" className={'bi app-icon ' + (filtered ? 'bi-funnel-fill' : 'bi-funnel')} />{t('ui.filter')}</Button></Popover.Trigger>
          <Popover.Content width="max-content" align="start"><Flex direction="column" gap="3"><Text size="2" weight="medium">{t('studio.filterMatches', { defaultValue: 'Filter matches' })}</Text>
        <Select.Root value={statusFilter} onValueChange={setStatusFilter}><Select.Trigger aria-label={t('studio.matchStatus', { defaultValue: 'Match status' })} /><Select.Content><Select.Item value="all">{t('studio.allStatuses', { defaultValue: 'All statuses' })}</Select.Item>{(Object.keys(STATUS_COLOR) as DemoMeta['status'][]).map(status => <Select.Item key={status} value={status}>{t(`demoList.status.${status}`)}</Select.Item>)}</Select.Content></Select.Root>
        <Flex align="center" gap="1">
          <DateField value={from} max={to || undefined} onChange={setFrom} label={t('demoList.from')} />
          <Text size="1" color="gray">
            –
          </Text>
          <DateField value={to} min={from || undefined} onChange={setTo} label={t('demoList.to')} />
          {(from || to) && (
            <IconButton
              size="1"
              variant="surface"
              color="gray"
              onClick={() => {
                setFrom('');
                setTo('');
              }}
              aria-label={t('demoList.clearDates')}
            >
              <i aria-hidden="true" className="bi bi-x-lg app-icon"  />
            </IconButton>
          )}
        </Flex>
        {filtered && <Button variant="soft" onClick={() => { setQuery(''); setFrom(''); setTo(''); setStatusFilter('all'); }}>{t('studio.clearFilters', { defaultValue: 'Clear filters' })}</Button>}
          </Flex></Popover.Content>
        </Popover.Root>
      </Flex>;

  return (
    <Flex direction="column" className="studio-match-library" style={{ flex: 1, minHeight: 0 }}>
      {refreshStatus && <Toast message={t(`demoList.${refreshStatus}`)} color={refreshStatus === 'refreshFailed' ? 'red' : 'green'}
        duration={refreshStatus === 'refreshed' ? 3000 : 0} onDismiss={() => setRefreshStatus(undefined)} />}
      {toolbar ? createPortal(actions, toolbar) : <Box mb="4">{actions}</Box>}
      <Text size="2" color="gray" mb="3">{t('studio.matchCount', { defaultValue: '{{shown}} of {{total}} matches', shown: visible.length, total: demos.length })}</Text>

      <div ref={scrollRef} className="demo-scroll">
        {visible.length > 0 ? (
          <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
            {virtualizer.getVirtualItems().map((row) => {
              const d = visible[row.index]!;
              const exports = exportsByDemo.get(d.id);
              return (

                  <button key={d.id} data-index={row.index} ref={virtualizer.measureElement} type="button" aria-pressed={d.id === selectedId} className={'demo-item ' + (d.id === selectedId ? 'active' : '')} style={{ position: 'absolute', top: 0, left: 0, width: '100%', minHeight: rowHeight, transform: 'translateY(' + row.start + 'px)' }} onClick={() => onSelect(d.id)}>
                    <span className="demo-chip">{d.mapName?.replace(/^de_/, '').slice(0, 2) ?? '—'}</span>
                    <span className="demo-copy">
                      <span className="demo-name"><Text weight="bold" truncate>{d.mapName ?? d.name.replace(/\.dem$/i, '')}</Text>
                        {d.summary && <span className="demo-score"><span>{d.summary.scoreA}</span><i>:</i><span>{d.summary.scoreB}</span></span>}
                      </span>
                      <span className="demo-date mono">{format(demoDate(d), 'yyyy-MM-dd HH:mm')}</span>
                      <span className="demo-states">
                        <span>{t('demoList.videos', { n: exports?.videos ?? 0 })}</span>
                        <span className="demo-job-states">
                        <Badge size="1" color={STATUS_COLOR[d.status]}>{t(`demoList.status.${d.status}`)}</Badge>
                        {exports && exports.running > 0 && <span className="queue-note">{t('demoList.exportJobs', { status: t('renders.status.running'), n: exports.running })}</span>}
                        {exports && exports.queued > 0 && <span className="queue-note">{t('demoList.exportJobs', { status: t('renders.status.queued'), n: exports.queued })}</span>}
                        {exports && exports.error > 0 && <Badge color="red"><i aria-hidden="true" className="bi bi-exclamation-triangle app-icon"  />{exports.error}</Badge>}
                        </span>
                      </span>
                    </span><i aria-hidden="true" className="bi bi-chevron-right demo-chevron app-icon" />
                  </button>
              );
            })}
          </div>
        ) : (
          <Box p="5" className="studio-empty-state"><i aria-hidden="true" className="bi bi-collection-play studio-empty-icon" /><Heading size="4" mt="2">{demos.length === 0 ? t('studio.noMatches', { defaultValue: 'Your match library starts here' }) : t('studio.noMatchesFound', { defaultValue: 'No matching demos' })}</Heading>
            <Text as="p" size="2" color="gray" mt="2">
              {demos.length === 0 ? t('demoList.empty') : t('demoList.noMatch')}
            </Text>
            <Button mt="4" variant="soft" onClick={() => demos.length === 0 ? void addFiles() : (setQuery(''), setFrom(''), setTo(''), setStatusFilter('all'))}>{demos.length === 0 ? t('studio.importDemo', { defaultValue: 'Import demo' }) : t('studio.clearFilters', { defaultValue: 'Clear filters' })}</Button>
          </Box>
        )}
      </div>
    </Flex>
  );
}
