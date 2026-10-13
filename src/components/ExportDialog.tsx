import { summarizeHighlights } from '../highlightWindows.ts';
import { useEffect, useState, type ReactNode } from 'react';
import { Badge, Box, Button, Callout, Dialog, Flex, Grid, Heading, SegmentedControl, Select, Slider, Switch, Text } from '@radix-ui/themes';
import { useTranslation } from 'react-i18next';
import { api, errorText, DEFAULT_RENDER_OPTIONS, RENDER_QUALITY, type Highlight, type RenderOptions, type Status } from '../api.ts';

const RESOLUTIONS = [
  { label: '720p', width: 1280, height: 720 },
  { label: '1080p', width: 1920, height: 1080 },
  { label: '1440p', width: 2560, height: 1440 },
  { label: '4K', width: 3840, height: 2160 },
];
const HUD_TOGGLES = ['hud', 'crosshair', 'radar', 'killFeed', 'chat', 'viewmodel', 'tracers', 'xray', 'trueView'] as const;

export function ExportDialog({ open, onOpenChange, count, seconds, status, onSetup, onSubmit, onSubmitted, forceMerge = false, disabled = false, children, highlights, tickRate }: {
  open: boolean; onOpenChange: (open: boolean) => void; count: number; seconds: number; status?: Status; onSetup: () => void;
  highlights?: Highlight[]; tickRate?: number;
  onSubmit: (options: RenderOptions) => Promise<unknown>; onSubmitted: () => void; forceMerge?: boolean; disabled?: boolean; children?: ReactNode;
}) {
  const { t } = useTranslation();
  const [opts, setOpts] = useState<RenderOptions>(DEFAULT_RENDER_OPTIONS);
  const [submitting, setSubmitting] = useState(false);
  const [loadingDefaults, setLoadingDefaults] = useState(false);
  const [defaultsFailed, setDefaultsFailed] = useState(false);
  const [defaultsAttempt, setDefaultsAttempt] = useState(0);
  const [error, setError] = useState<string>();
  useEffect(() => {
    if (!open) return;
    let alive = true;
    setError(undefined);
    setLoadingDefaults(true);
    setDefaultsFailed(false);
    void api.settings().then(response => {
      if (alive) setOpts({ ...DEFAULT_RENDER_OPTIONS, ...response.settings.renderDefaults, extraLaunchOptions: [...(response.settings.renderDefaults?.extraLaunchOptions ?? DEFAULT_RENDER_OPTIONS.extraLaunchOptions)] });
    }).catch(reason => { if (alive) { setError(errorText(reason)); setDefaultsFailed(true); } }).finally(() => { if (alive) setLoadingDefaults(false); });
    return () => { alive = false; };
  }, [open, defaultsAttempt]);
  const preview = highlights && tickRate ? summarizeHighlights(highlights, tickRate, opts.keyMomentsOnly) : undefined;
  const fullHud = opts.hud && opts.crosshair && opts.radar && opts.killFeed && opts.chat;
  const render = async () => {
    if (submitting || loadingDefaults) return;
    setSubmitting(true); setError(undefined);
    try {
      await onSubmit({ ...opts, roundResultLabel: t('highlights.roundResult'), merge: forceMerge || opts.merge, keyMomentsOnly: !!highlights && opts.keyMomentsOnly });
      onOpenChange(false); onSubmitted();
    } catch (reason) { setError(errorText(reason)); } finally { setSubmitting(false); }
  };
  return <Dialog.Root open={open} onOpenChange={value => { if (!submitting) onOpenChange(value); }}>
    <Dialog.Content maxWidth="860px" className="studio-export-dialog">
      <Dialog.Title>{t('studio.exportTitle', { defaultValue: 'Make your clips' })}</Dialog.Title>
      <Dialog.Description size="2" color="gray">{t('highlights.summary', { n: count, seconds: Math.round(preview?.seconds ?? seconds) })}</Dialog.Description>
      {error && <Callout.Root color="red" mt="3" role="alert"><Callout.Text>{error}</Callout.Text>{defaultsFailed && <Button variant="soft" onClick={() => setDefaultsAttempt(attempt => attempt + 1)}>{t('studio.retry', { defaultValue: 'Retry' })}</Button>}</Callout.Root>}
      <Box className="studio-export-summary" mt="4" p="4" aria-live="polite"><Flex gap="2" wrap="wrap" mb="2"><Badge size="2">{opts.width} × {opts.height}</Badge><Badge size="2">{opts.fps} FPS</Badge><Badge size="2" color="gray">{fullHud ? t('studio.fullHud', { defaultValue: 'Full HUD & chat' }) : t('studio.customHud', { defaultValue: 'Custom HUD' })}</Badge><Badge size="2" color="gray">{opts.voice ? t('studio.voiceIncluded', { defaultValue: 'Voice, when recorded' }) : t('studio.voiceOff', { defaultValue: 'Voice off' })}</Badge></Flex><Text size="2" color="gray">{t('studio.originalDemoExport', { defaultValue: 'Recorded from your original demo. These options apply to this export.' })}</Text>
        {highlights && <details className="studio-selected-moments"><summary>{t('studio.selectedMoments', { defaultValue: 'Selected moments ({{count}})', count })}</summary><ul>{highlights.map(highlight => <li key={highlight.id}>{highlight.title}</li>)}</ul></details>}
      </Box>
      <fieldset className="studio-export-options" disabled={loadingDefaults || defaultsFailed || submitting} aria-busy={loadingDefaults} style={loadingDefaults ? { opacity: 0.65 } : undefined}>
      {children}
        <Grid className="studio-export-basics" gap="4" mt="4">
          <Box><Text as="div" size="2" weight="medium" mb="2">{t('common.resolution')}</Text><SegmentedControl.Root value={String(opts.width)} onValueChange={value => { const resolution = RESOLUTIONS.find(item => item.width === Number(value)); if (resolution) setOpts({ ...opts, width: resolution.width, height: resolution.height }); }} aria-label={t('common.resolution')} style={{ width: '100%' }}>{RESOLUTIONS.map(resolution => <SegmentedControl.Item key={resolution.width} value={String(resolution.width)} title={`${resolution.width} × ${resolution.height}`}>{resolution.label}</SegmentedControl.Item>)}</SegmentedControl.Root></Box>
          <Box><Text as="div" size="2" weight="medium" mb="2">{t('studio.frameRate', { defaultValue: 'Frame rate' })}</Text><SegmentedControl.Root value={String(opts.fps)} onValueChange={value => setOpts({ ...opts, fps: Number(value) })} aria-label={t('studio.frameRate', { defaultValue: 'Frame rate' })} style={{ width: '100%' }}>{[30, 60, 120].map(fps => <SegmentedControl.Item key={fps} value={String(fps)}>{fps} FPS</SegmentedControl.Item>)}</SegmentedControl.Root></Box>
          <Box><Text as="div" size="2" weight="medium" mb="2">{t('highlights.sizeLimit')}{count > 1 && !forceMerge && (opts.merge ? t('highlights.sizeMerged') : t('highlights.sizeEach'))}</Text><SegmentedControl.Root value={String(opts.maxSizeMb)} onValueChange={value => setOpts({ ...opts, maxSizeMb: value === 'null' ? null : Number(value) })} aria-label={t('highlights.sizeLimit')} style={{ width: '100%' }}>{[10, 20, 50].map(size => <SegmentedControl.Item key={size} value={String(size)}>{size} MB</SegmentedControl.Item>)}<SegmentedControl.Item value="null">{t('highlights.unlimited')}</SegmentedControl.Item></SegmentedControl.Root></Box>
          <Flex direction="column" gap="3" justify="center">
            {highlights && <Text as="label" size="2"><Flex gap="2" align="center"><Switch checked={opts.keyMomentsOnly} onCheckedChange={keyMomentsOnly => setOpts({ ...opts, keyMomentsOnly })} />{t('highlights.keyMomentsOnly')}</Flex></Text>}
            {count > 1 && !forceMerge && <Text as="label" size="2"><Flex gap="2" align="center"><Switch checked={opts.merge} onCheckedChange={merge => setOpts({ ...opts, merge })} />{t('highlights.merge')}</Flex></Text>}
            <Text as="label" size="2"><Flex gap="2" align="center"><Switch checked={opts.voice} onCheckedChange={voice => setOpts({ ...opts, voice })} />{t('studio.voiceWhenRecorded', { defaultValue: 'Voice chat, when recorded' })}</Flex></Text>
          </Flex>
        </Grid>
        {highlights && !opts.keyMomentsOnly && opts.maxSizeMb != null && <Text as="p" role="status" size="2" color="amber" mt="3">{t('highlights.fullLengthWarning')}</Text>}
        <details className="studio-export-advanced">
          <summary>{t('studio.advancedCapture', { defaultValue: 'Advanced · encoder, HUD & capture' })}</summary>
          <Grid className="studio-export-advanced-grid" gap="5" mt="4" align="start">
            <Flex direction="column" gap="4"><Box><Text as="label" htmlFor="export-encoder" size="2" weight="medium" mb="2">{t('highlights.encoder')}</Text><Select.Root value={opts.codec} onValueChange={codec => setOpts({ ...opts, codec, crf: RENDER_QUALITY[codec] ?? DEFAULT_RENDER_OPTIONS.crf })}><Select.Trigger id="export-encoder" aria-label={t('highlights.encoder')} style={{ width: '100%' }} /><Select.Content><Select.Item value="h264_amf">H.264 · AMD</Select.Item><Select.Item value="hevc_amf">H.265 · AMD</Select.Item><Select.Item value="libx264">H.264 · CPU</Select.Item><Select.Item value="libx265">H.265 · CPU</Select.Item><Select.Item value="h264_nvenc">H.264 · NVIDIA</Select.Item><Select.Item value="hevc_nvenc">H.265 · NVIDIA</Select.Item></Select.Content></Select.Root></Box>
              <Box><Text as="label" htmlFor="export-file-format" size="2" weight="medium" mb="2">{t('studio.fileFormat',{defaultValue:'File format'})}</Text><Select.Root value={opts.container} disabled={loadingDefaults||defaultsFailed||submitting} onValueChange={container=>setOpts({...opts,container})}><Select.Trigger id="export-file-format" aria-label={t('studio.fileFormat',{defaultValue:'File format'})} style={{width:'100%'}}/><Select.Content><Select.Item value="mp4">MP4</Select.Item><Select.Item value="mkv">MKV</Select.Item></Select.Content></Select.Root></Box>
              <Box><Text as="div" size="2" weight="medium" mb="2">{t('highlights.hudScale', { n: opts.hudScale.toFixed(2) })}</Text><Slider disabled={loadingDefaults || defaultsFailed || submitting} aria-label={t('highlights.hudScale', { n: opts.hudScale.toFixed(2) })} min={0.5} max={0.95} step={0.05} value={[opts.hudScale]} onValueChange={([value]) => setOpts({ ...opts, hudScale: value ?? 0.85 })} /></Box>
              <Box><Text as="label" size="2"><Flex align="center" gap="2"><Switch checked={!opts.showGame} onCheckedChange={hideGame => setOpts({ ...opts, showGame: !hideGame })} />{t('highlights.hideGame')}</Flex></Text><Text as="p" size="2" color="gray" mt="2">{t(opts.showGame ? 'highlights.visibleGame' : 'highlights.hiddenGame')}</Text></Box>
            </Flex>
            <Flex direction="column" gap="3"><Heading size="3">{t('studio.onScreen', { defaultValue: 'On-screen elements' })}</Heading>{HUD_TOGGLES.map(key => <Text as="label" size="2" key={key}><Flex align="center" justify="between" gap="3"><Text>{t(`highlights.toggle.${key}`)}</Text><Switch style={{ flexShrink: 0 }} checked={opts[key]} disabled={key === 'crosshair' && !opts.hud} onCheckedChange={value => setOpts({ ...opts, [key]: value })} /></Flex></Text>)}</Flex>
          </Grid>
        </details>
      </fieldset>
      <Flex gap="3" justify="end" align="center" wrap="wrap" mt="5">
        {status && !status.ok && <Button className="environment-notice" variant="soft" color="red" style={{ marginRight: 'auto' }} onClick={() => { onOpenChange(false); onSetup(); }}><i aria-hidden="true" className="bi bi-exclamation-triangle app-icon" />{status.missingRenderTools.length ? t('common.missingTools', { tools: status.missingRenderTools.join(', ') }) : t('highlights.notReady')}<i aria-hidden="true" className="bi bi-arrow-right app-icon" /></Button>}
        <Dialog.Close><Button variant="soft" color="gray" disabled={submitting}>{t('common.cancel')}</Button></Dialog.Close>
        <Button disabled={submitting || loadingDefaults || defaultsFailed || !status?.ok || disabled || count === 0} onClick={() => void render()}>{loadingDefaults ? t('studio.loadingDefaults', { defaultValue: 'Loading defaults…' }) : submitting ? t('highlights.submitting') : t('studio.exportClips', { defaultValue: 'Export {{count}} clip(s)', count })}</Button>
      </Flex>
    </Dialog.Content>
  </Dialog.Root>;
}
