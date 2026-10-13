import { useNotify, ConfirmDialog } from './Notifications.tsx';
import { Spinner } from './Spinner.tsx';
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { Badge, Box, Button, Callout, Card, DataList, Dialog, DropdownMenu, Flex, Heading, IconButton, Link, Progress, Text } from '@radix-ui/themes';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { api, errorText, mb, type ClipMedia, type JobOutput, type ParsedDemo, type RenderJob } from '../api.ts';
import { LogView } from './LogView.tsx';
import { fmtDateTime, translateError, translateRenderStage } from '../i18n/index.ts';
import { ClipPlayer } from './studio/ClipPlayer.tsx';
import { useClipMedia } from './studio/ClipLibrary.tsx';
import { useStudioAction } from './studio/useStudioAction.ts';
import { isReviewPreview } from '../renderPurpose.ts';

interface PreviewSelection { jobId: string; output: JobOutput; title: string; media?: ClipMedia }
function OutputPoster({ job, output, title, onPreview }: { job: RenderJob; output: JobOutput; title: string; onPreview: (selection: PreviewSelection) => void }) {
  const { t } = useTranslation();
  const notify = useNotify();
  const ref = useRef<HTMLButtonElement>(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => { const element = ref.current; if (!element) return; const observer = new IntersectionObserver(entries => setVisible(entries.some(entry => entry.isIntersecting))); observer.observe(element); return () => observer.disconnect(); }, []);
  const { media, error } = useClipMedia({ key: `${job.id}|${output.file}`, job, output, category: 'other', tags: [] }, visible);
  return <Box style={{ minWidth: 0 }} className="studio-output-card"><button ref={ref} className="studio-output-poster" type="button" aria-label={t('studio.previewClip', { defaultValue: 'Preview {{title}}', title })} onClick={() => onPreview({ jobId: job.id, output, title, media })}>{media?.posterPath ? <img loading="lazy" src={api.fileSrc(media.posterPath)} alt="" /> : <span className="studio-output-placeholder"><i aria-hidden="true" className="bi bi-camera-video" />{error ? t('studio.previewUnavailable', { defaultValue: 'Poster unavailable' }) : t('studio.recordedClip', { defaultValue: 'Recorded clip' })}</span>}<span className="studio-output-play"><i aria-hidden="true" className="bi bi-play-fill" /></span></button><Text as="div" size="2" mt="2" weight="medium" truncate title={title}>{title}</Text><Text as="div" size="1" color="gray" mt="1">{media ? `${media.width} × ${media.height} · ${Math.round(media.fps)} FPS · ${media.durationSeconds.toFixed(1)}s` : t('studio.requestedFormat', { defaultValue: 'Requested {{height}}p · {{fps}} FPS', height: job.options.height, fps: job.options.fps })}</Text><Text as="div" size="1" color="gray" mt="1">{mb(output.bytes)} · <Link size="1" href="#" onClick={event => { event.preventDefault(); void api.open(output.file).catch(reason => notify(errorText(reason))); }}>{t('renders.openInPlayer')} <i aria-hidden="true" className="bi bi-box-arrow-up-right app-icon" /></Link></Text></Box>;
}
const COLOR: Record<RenderJob['status'], 'gray' | 'amber' | 'green' | 'red'> = { queued: 'amber', running: 'amber', done: 'green', partial: 'amber', error: 'red', cancelled: 'gray' };

function duration(job: RenderJob, t: TFunction, now: number): string | undefined {
  if (!job.startedAt) return undefined;
  const end = job.finishedAt ? new Date(job.finishedAt) : new Date(now);
  const sec = Math.max(0, Math.round((end.getTime() - new Date(job.startedAt).getTime()) / 1000));
  return sec < 60 ? t('common.seconds', { n: sec }) : t('common.minutesSeconds', { m: Math.floor(sec / 60), s: sec % 60 });
}

function JobCard({ job, parsed, onChanged, onPreview, onOpenMoments }: { job: RenderJob; parsed: ParsedDemo; onChanged: () => Promise<void>; onPreview: (selection: PreviewSelection) => void; onOpenMoments?: () => void }) {
  const { t } = useTranslation();
  const notify = useNotify();
  const [logOpen, setLogOpen] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const actions = useStudioAction();
  const cancelling = actions.pending.has('cancel:' + job.id) || job.stage === 'cancelling';
  const retrying = actions.pending.has('retry:' + job.id);
  const titleOf = (id?: string) => (job.sourceSnapshot?.highlights ?? job.analysisClips?.highlights ?? parsed.highlights).find((h) => h.id === id)?.title;
  const act = (fn: () => Promise<unknown>) => () => void fn().then(onChanged).catch((e) => notify(errorText(e)));
  const active = job.status === 'queued' || job.status === 'running';
  const cancel = () => actions.run('cancel:' + job.id, async () => { if (job.stage === 'cancelling') return; try { await api.cancel(job.id); await onChanged(); } catch (reason) { notify(errorText(reason)); } });
  const retry = () => actions.run('retry:' + job.id, async () => { try { await api.retryRender(job.id); await onChanged(); } catch (reason) { notify(errorText(reason)); } });
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (job.status !== 'running' || job.finishedAt) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [job.status, job.finishedAt]);
  const pct = job.progress === undefined ? undefined : Math.min(99, Math.floor(job.progress * 100));
  const total = job.outputs.reduce((s, o) => s + o.bytes, 0);
  const revealTarget = job.outputs[0]?.file;
  const hasOptions = job.options.trueView || !job.options.hud || !job.options.crosshair || !job.options.radar || !job.options.killFeed || !job.options.viewmodel || !job.options.tracers || job.options.chat || job.options.xray || job.options.voice;

  return (
    <Card className="job-card" data-job-id={job.id}>
      <Flex align="center" gap="3" wrap="wrap" className="job-header">
        {isReviewPreview(job) && <Badge color="gray">{t('studio.reviewPreview',{defaultValue:'Review preview'})}</Badge>}
        <Badge color={COLOR[job.status]} size="2">
          {t(`renders.status.${job.status}`)}
        </Badge>
        <Heading data-text-role="body-heading" size="3">{fmtDateTime(job.createdAt)}</Heading>
        <Text size="2" color="gray">
          {job.analysisClips && `${job.analysisClips.title} · `}{t('renders.summary', { n: job.highlightIds.length })} · {job.options.maxSizeMb ? t('renders.sizeLimit', { mb: job.options.maxSizeMb }) : t('renders.noSizeLimit')} · {t('studio.requestedFormat', { defaultValue: 'Requested {{height}}p · {{fps}} FPS', height: job.options.height, fps: job.options.fps })}
        </Text>
        <Box style={{ flex: 1 }} />
        {active && (
          <Button size="2" variant="soft" color="red" disabled={cancelling} onClick={() => void cancel()}>
            {cancelling ? t('studio.cancelling', { defaultValue: 'Cancelling…' }) : t('common.cancel')}
          </Button>
        )}
        {['error', 'partial', 'cancelled'].includes(job.status) && (job.sourceSnapshot || job.analysisClips ? <Button size="2" variant="soft" disabled={retrying} onClick={() => void retry()}>{retrying ? t('studio.retrying', { defaultValue: 'Queuing retry…' }) : t('studio.retryExport', { defaultValue: 'Retry export' })}</Button> : onOpenMoments && <Button size="2" variant="soft" onClick={onOpenMoments}>{t('studio.openMatchExportAgain', { defaultValue: 'Open moments to export again' })}</Button>)}
        {revealTarget && (
          <Button size="1" variant="soft" onClick={() => void api.reveal(revealTarget).catch(reason => notify(errorText(reason)))}>
            <i aria-hidden="true" className="bi bi-folder2-open app-icon"  /> {t('renders.openFolder')}
          </Button>
        )}
        <DropdownMenu.Root>
          <DropdownMenu.Trigger>
            <IconButton size="1" variant="soft" aria-label={t('common.more')}>
              <i aria-hidden="true" className="bi bi-three-dots app-icon"  />
            </IconButton>
          </DropdownMenu.Trigger>
          <DropdownMenu.Content align="end">
            <DropdownMenu.Item onSelect={() => setLogOpen(true)}>
              <i aria-hidden="true" className="bi bi-file-earmark-text app-icon"  /> {t('renders.viewLog')}
            </DropdownMenu.Item>
            {!active && (
              <>
                <DropdownMenu.Separator />
                <DropdownMenu.Item color="red" onSelect={() => setConfirmDelete(true)}>
                  {t('renders.deleteJob')}
                </DropdownMenu.Item>
              </>
            )}
          </DropdownMenu.Content>
        </DropdownMenu.Root>
      </Flex>

      {active && (
        <Box className="job-content">
          <Flex justify="between" align="center" mb="2" gap="3" wrap="wrap">
            <Flex align="center" gap="2">
              <Spinner size="1" />
              <Text size="2" color="amber">
                {cancelling ? t('studio.cancelling', { defaultValue: 'Cancelling…' }) : job.stage ? translateRenderStage(job.stage) : job.status === 'queued' ? t('renders.waitingPrevious') : t('renders.starting')}
              </Text>
            </Flex>
            <Text size="1" color="gray">
              {duration(job, t, now)}
            </Text>
          </Flex>
          {job.status === 'running' && <>
            <Flex justify="between" mb="1">
              <Text size="1" color="gray">{t('renders.overallProgress')}</Text>
              <Text size="1" style={{ fontVariantNumeric: 'tabular-nums' }}>{pct === undefined ? '—' : `${pct}%`}</Text>
            </Flex>
            <Progress value={pct} color="amber" aria-label={t('renders.overallProgress')} />
          </>}
        </Box>
      )}

      {!!job.failedHighlights?.length && (
        <Callout.Root color="amber" size="1" mt="3">
          <Callout.Text>{t('renders.failedHighlights', { titles: job.failedHighlights.join('、') })}</Callout.Text>
        </Callout.Root>
      )}
      {(job.error || job.errorCode) && (
        <Callout.Root color="red" size="1" mt="3">
          <Callout.Text className="selectable">{translateError(job.error, job.errorCode)}</Callout.Text>
        </Callout.Root>
      )}

      {job.outputs.length > 0 && (
        <div className="job-outputs">
          <div className="job-previews">
            {job.outputs.map((o) => {
              const title = o.isFinal ? t('renders.merged') : (titleOf(o.highlightId) ?? o.title);
              return (
                <OutputPoster key={o.file} job={job} output={o} title={title} onPreview={onPreview} />
              );
            })}
          </div>
          <DataList.Root size="1" className="job-details">
            <DataList.Item>
              <DataList.Label minWidth="0" style={{ flexBasis: 90 }}>{t('renders.files')}</DataList.Label>
              <DataList.Value>{job.outputs.length}</DataList.Value>
            </DataList.Item>
            <DataList.Item>
              <DataList.Label minWidth="0" style={{ flexBasis: 90 }}>{t('renders.totalSize')}</DataList.Label>
              <DataList.Value>{mb(total)}</DataList.Value>
            </DataList.Item>
            <DataList.Item>
              <DataList.Label minWidth="0" style={{ flexBasis: 90 }}>{t('renders.elapsed')}</DataList.Label>
              <DataList.Value>{duration(job, t, now) ?? '—'}</DataList.Value>
            </DataList.Item>
            <DataList.Item>
              <DataList.Label minWidth="0" style={{ flexBasis: 90 }}>{t('studio.requestedQuality', { defaultValue: 'Requested quality' })}</DataList.Label>
              <DataList.Value>
                {job.options.width}×{job.options.height} @ {job.options.fps}
              </DataList.Value>
            </DataList.Item>
            {hasOptions && <DataList.Item>
              <DataList.Label minWidth="0" style={{ flexBasis: 90 }}>{t('renders.options')}</DataList.Label>
              <DataList.Value>
                <Flex gap="1" wrap="wrap">
                  {job.options.trueView && <Badge size="1">TrueView</Badge>}
                  {!job.options.hud && <Badge size="1">{t('renders.badge.noHud')}</Badge>}
                  {job.options.hud && !job.options.crosshair && <Badge size="1">{t('renders.badge.noCrosshair')}</Badge>}
                  {!job.options.radar && <Badge size="1">{t('renders.badge.noRadar')}</Badge>}
                  {!job.options.killFeed && <Badge size="1">{t('renders.badge.noKillFeed')}</Badge>}
                  {!job.options.viewmodel && <Badge size="1">{t('renders.badge.noViewmodel')}</Badge>}
                  {!job.options.tracers && <Badge size="1">{t('renders.badge.noTracers')}</Badge>}
                  {job.options.chat && <Badge size="1">{t('renders.badge.chat')}</Badge>}
                  {job.options.xray && <Badge size="1">X-ray</Badge>}
                  {job.options.voice && <Badge size="1">{t('renders.badge.voice')}</Badge>}
                </Flex>
              </DataList.Value>
            </DataList.Item>}
            <DataList.Item>
              <DataList.Label minWidth="0" style={{ flexBasis: 90 }}>{t('renders.location')}</DataList.Label>
              <DataList.Value>
                <Text className="mono selectable" style={{ overflowWrap: 'anywhere' }}>
                  {job.outputs[0]?.file.replace(/[\\/][^\\/]+$/, '')}
                </Text>
              </DataList.Value>
            </DataList.Item>
          </DataList.Root>
        </div>
      )}

      <Dialog.Root open={logOpen} onOpenChange={setLogOpen}>
        <Dialog.Content maxWidth="900px">
          <Dialog.Title>{t('renders.logTitle')}</Dialog.Title>
          <Dialog.Description size="2" color="gray">
            {fmtDateTime(job.createdAt)} · {t(`renders.status.${job.status}`)}
          </Dialog.Description>
          <LogView lines={job.log} empty={t('renders.logEmpty')} />
          <Flex justify="end" mt="3">
            <Dialog.Close>
              <Button variant="soft">{t('common.close')}</Button>
            </Dialog.Close>
          </Flex>
        </Dialog.Content>
      </Dialog.Root>

      <ConfirmDialog open={confirmDelete} onOpenChange={setConfirmDelete} title={t('renders.deleteTitle')}
        description={t('renders.deleteBody', { n: job.outputs.length })}
        confirmLabel={t('common.delete')} onConfirm={act(() => api.deleteJob(job.id))} />
    </Card>
  );
}

export function RendersTab({ jobs, parsed, onChanged, scrollRef, onOpenMoments }: { jobs: RenderJob[]; parsed: ParsedDemo; onChanged: () => Promise<void>; scrollRef: RefObject<HTMLDivElement | null>; onOpenMoments?: () => void }) {
  const { t } = useTranslation();
  const notify = useNotify();
  const listRef = useRef<HTMLDivElement>(null);
  const [scrollMargin, setScrollMargin] = useState(0);
  const [preview, setPreview] = useState<PreviewSelection>();
  const previewJob = jobs.find(job => job.id === preview?.jobId);
  const { media: measuredPreview } = useClipMedia(preview && previewJob ? { key: `${preview.jobId}|${preview.output.file}`, job: previewJob, output: preview.output, category: 'other', tags: [] } : undefined, !!preview);
  const previewMedia = measuredPreview ?? preview?.media;
  useEffect(() => { if (preview && !jobs.some(job => job.outputs.some(output => output.file === preview.output.file))) setPreview(undefined); }, [jobs, preview]);
  const hasJobs = jobs.length > 0;
  const getItemKey = useCallback((index: number) => jobs[index]!.id, [jobs]);
  const virtualizer = useVirtualizer({
    useAnimationFrameWithResizeObserver: true,
    count: jobs.length,
    getScrollElement: () => scrollRef.current,
    getItemKey,
    estimateSize: (index) => jobs[index]!.outputs.length > 0 ? 320 : 110,
    overscan: 3,
    gap: 12,
    scrollMargin,
  });

  useLayoutEffect(() => {
    const scroll = scrollRef.current;
    const list = listRef.current;
    if (!scroll || !list) return;
    let width = scroll.clientWidth;
    const measureLayout = () => {
      // Account for the tab body's padding without introducing a second scrollbar.
      setScrollMargin(list.getBoundingClientRect().top - scroll.getBoundingClientRect().top + scroll.scrollTop - scroll.clientTop);
      if (scroll.clientWidth !== width) {
        width = scroll.clientWidth;
        // Off-screen measurements also become stale when cards reflow at a new width.
        virtualizer.measure();
      }
    };
    measureLayout();
    const observer = new ResizeObserver(measureLayout);
    observer.observe(scroll);
    return () => observer.disconnect();
  }, [hasJobs, scrollRef, virtualizer]);

  if (!hasJobs) {
    return <Box className="studio-empty-state" p="5"><i aria-hidden="true" className="bi bi-camera-video studio-empty-icon" /><Heading size="4" mt="2">{t('studio.noExports', { defaultValue: 'Your clips will appear here' })}</Heading><Text as="p" size="2" color="gray" mt="2">{t('renders.empty')}</Text>{onOpenMoments && <Button mt="4" onClick={onOpenMoments}>{t('studio.chooseMoments', { defaultValue: 'Choose moments' })}</Button>}</Box>;
  }
  return (
    <><Flex align="center" gap="3" mb="3"><Heading data-text-role="subtitle" size="3">{t('demoView.tabs.videos')}</Heading><Text size="1" color="gray">{t('demoList.videos', { n: jobs.reduce((n, job) => n + job.outputs.length, 0) })} · {mb(jobs.reduce((n, job) => n + job.outputs.reduce((bytes, output) => bytes + output.bytes, 0), 0))}</Text></Flex>
    <div ref={listRef} style={{ height: virtualizer.getTotalSize(), position: 'relative', overflowAnchor: 'none' }}>
      {virtualizer.getVirtualItems().map((row) => (
        <div
          key={row.key}
          data-index={row.index}
          // Reattach after renders so measure() also refreshes unchanged mounted cards.
          ref={(element) => virtualizer.measureElement(element)}
          style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${row.start - scrollMargin}px)` }}
        >
          <JobCard job={jobs[row.index]!} parsed={parsed} onChanged={onChanged} onPreview={setPreview} onOpenMoments={onOpenMoments} />
        </div>
      ))}
    </div><Dialog.Root open={!!preview} onOpenChange={open => { if (!open) setPreview(undefined); }}><Dialog.Content maxWidth="1120px" className="studio-clip-preview"><Dialog.Title>{preview?.title}</Dialog.Title><Dialog.Description size="2" mb="3">{previewMedia ? `${previewMedia.width} × ${previewMedia.height} · ${Math.round(previewMedia.fps)} FPS · ${previewMedia.durationSeconds.toFixed(1)}s` : t('studio.savedRecording', { defaultValue: 'Saved recording' })}</Dialog.Description>{preview && <ClipPlayer src={api.fileSrc(preview.output.file)} title={preview.title} poster={previewMedia?.posterPath ? api.fileSrc(previewMedia.posterPath) : undefined} />}<Flex gap="3" justify="end" mt="4" wrap="wrap">{preview && <Button variant="soft" onClick={() => void api.reveal(preview.output.file).catch(reason => notify(errorText(reason)))}>{t('renders.openFolder')}</Button>}<Dialog.Close><Button variant="soft">{t('common.close')}</Button></Dialog.Close></Flex></Dialog.Content></Dialog.Root></>
  );
}
