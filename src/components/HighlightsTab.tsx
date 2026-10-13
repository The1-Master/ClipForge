import { ExportDialog } from './ExportDialog.tsx';
import { displayPlayerName } from '../playerName.ts';
import { useEffect, useMemo, useState } from 'react';
import { Badge, Box, Button, Callout, Checkbox, Dialog, Flex, Heading, SegmentedControl, Select, Slider, Table, Text } from '@radix-ui/themes';
import { useTranslation } from 'react-i18next';
import { api, clock, type DemoMeta, type Highlight, type ParsedDemo, type Status } from '../api.ts';
import { automationApi, type AutomationStatus, type FunnyMoment, type ReviewMode } from '../api_automation.ts';
import { FeedbackButton } from './studio/FeedbackDialog';
import { DiscoveryMoments } from './studio/DiscoveryMoments';
import { ReviewControls } from './studio/ReviewControls';

const TAG_LABELS: Record<string, string> = { 'ninja-defuse': 'highlights.possibleNinja', 'post-round': 'highlights.postRound', posthumous: 'highlights.posthumous' };
const SCORE_PARTS = ['multikill', 'specialKills', 'posthumous', 'clutch', 'ninjaDefuse', 'context'] as const;
const HOT_TAGS = new Set(['ace', '4k', 'clutch', 'knife', 'noscope']);
type MomentCategory = 'highlights' | 'deaths' | 'ai' | 'discovery';
interface MomentState { player: string; playerChanged: boolean; minScore: number; category: MomentCategory; selected: Set<string> }
const matchState = new Map<string, MomentState>();

function summaryOf(h: Highlight): string {
  return h.title.replace(`${displayPlayerName(h.player.name)} — `, '').replace(/ · R\d+$/, '');
}

interface EvidenceFrameData { file: string; videoSeconds: number; demoTick?: number; cameraState?:'living-first-person'|'personal-death-camera' }
function EvidenceFrame({ frame, tickRate }: { frame: EvidenceFrameData; tickRate: number }) {
  const { t } = useTranslation();
  const [unavailable, setUnavailable] = useState(false);
  return <figure>{unavailable ? <div className="studio-evidence-unavailable" style={{ aspectRatio: '16/9', display: 'grid', placeItems: 'center', background: 'var(--app-background)', borderRadius: 8 }}><Text size="2" color="gray">{t('studio.evidenceImageMissing', { defaultValue: 'Evidence image unavailable' })}</Text></div> : <img src={api.fileSrc(frame.file)} loading="lazy" alt={t('studio.evidenceFrame', { defaultValue: 'Gameplay evidence at {{seconds}} seconds', seconds: frame.videoSeconds.toFixed(1) })} onError={() => setUnavailable(true)} />}<figcaption>{typeof frame.demoTick === 'number' && Number.isFinite(frame.demoTick) && frame.demoTick >= 0 ? t('studio.evidenceMatchTime', { defaultValue: 'Match {{time}}', time: clock(frame.demoTick, tickRate) }) : `${frame.videoSeconds.toFixed(1)}s`}{frame.cameraState==='personal-death-camera'&&<> · {t('studio.personalDeathCamera',{defaultValue:'Personal death camera'})}</>}</figcaption></figure>;
}

function SuggestionEvidence({ moment, tickRate }: { moment: FunnyMoment; tickRate: number }) {
  const { t } = useTranslation();
  const frames = Array.isArray(moment.evidence) ? moment.evidence.filter((frame): frame is EvidenceFrameData => !!frame && typeof frame === 'object' && typeof frame.file === 'string' && typeof frame.videoSeconds === 'number' && Number.isFinite(frame.videoSeconds) && frame.videoSeconds >= 0 && /^j:\/cs2clipstudio\/state\/automation\/ai\/[a-f0-9]{64}\/evidence-source-ordinal-v1\/[a-f0-9]{32}\/source-frame-\d{8}\.jpg$/i.test(frame.file.replaceAll('\\', '/'))).slice(0, 12) : [];
  const facts=moment.cameraFacts;
  const personalDeathCount=facts?.source==='source2_observer_handle'&&facts.observerMode===2&&Number.isInteger(facts.personalDeathFrames)&&facts.personalDeathFrames>0?facts.personalDeathFrames:0;
  return <details className="studio-evidence"><summary>{t('studio.viewEvidence', { defaultValue: 'View evidence' })}</summary>{moment.reason && <Text as="p" size="2" mt="2">{moment.reason}</Text>}{personalDeathCount>0&&<Text as="p" size="2" mt="2">{t('studio.personalDeathFrames',{defaultValue:'Personal death camera: {{count}} measured frames',count:personalDeathCount})}</Text>}{frames.length ? <div className="studio-evidence-frames">{frames.map(frame => <EvidenceFrame key={frame.file} frame={frame} tickRate={tickRate} />)}</div> : <Text as="p" size="2" color="gray" mt="2">{t('studio.evidenceUnavailable', { defaultValue: 'Evidence images are unavailable here. Review the exported clip before sharing.' })}</Text>}</details>;
}

export function HighlightsTab({ meta, parsed, status, onRendered, onSetup, playerId, review, reviewBusy, onReviewMatch }: { meta: DemoMeta; parsed: ParsedDemo; status?: Status; onRendered: () => void; onSetup: () => void; playerId?: string; review?: AutomationStatus['review']; reviewBusy?: boolean; onReviewMatch?: (playerId: string, mode: ReviewMode) => void }) {
  const { t } = useTranslation();
  const preferredPlayer = playerId && parsed.stats.some(player => player.steamid === playerId) ? playerId : 'all';
  const [state, setState] = useState<MomentState>(() => matchState.get(meta.id) ?? { player: preferredPlayer, playerChanged: false, minScore: 3, category: 'highlights', selected: new Set() });
  const { player: playerFilter, minScore, selected, category } = state;
  const setSelected = (update: Set<string> | ((selected: Set<string>) => Set<string>)) => setState(current => ({ ...current, selected: typeof update === 'function' ? update(current.selected) : update }));
  useEffect(() => { matchState.set(meta.id, state); }, [meta.id, state]);
  useEffect(() => {
    const players = new Set(parsed.stats.map(player => player.steamid));
    const moments = new Set(parsed.highlights.map(highlight => highlight.id));
    setState(current => {
      const invalidPlayer = current.player !== 'all' && !players.has(current.player);
      const player = invalidPlayer || !current.playerChanged ? preferredPlayer : current.player;
      const selected = new Set([...current.selected].filter(id => moments.has(id)));
      if (player === current.player && selected.size === current.selected.size) return current;
      return { ...current, player, playerChanged: invalidPlayer ? false : current.playerChanged, selected };
    });
  }, [preferredPlayer, parsed.stats, parsed.highlights]);
  const [dialog, setDialog] = useState(false);
  const [aiDialog, setAiDialog] = useState(false);
  const [selectedAi, setSelectedAi] = useState<Set<number>>(new Set());
  const [detail, setDetail] = useState<Highlight | null>(null);
  const tr = parsed.info.tickRate;

  const visible = useMemo(() => parsed.highlights.filter((h) => (playerFilter === 'all' || h.player.steamid === playerFilter) && (category === 'deaths' ? h.tags.includes('death') : !h.tags.includes('death') && (h.score >= minScore || h.tags.includes('exceptional-shot')))), [parsed.highlights, playerFilter, minScore, category]);
  const toggle = (id: string) =>
    setSelected((s) => {
      const n = new Set(s);
      n.has(id) ? n.delete(id) : n.add(id);
      return n;
    });
  const allVisibleSelected = visible.length > 0 && visible.every((h) => selected.has(h.id));
  const chosen = parsed.highlights.filter((h) => selected.has(h.id));
  const selectedSeconds = chosen.reduce((s, h) => s + (h.endTick - h.startTick) / tr, 0);
  const visibleIds = useMemo(() => new Set(visible.map(highlight => highlight.id)), [visible]);
  const hiddenCount = chosen.filter(highlight => !visibleIds.has(highlight.id)).length;
  const matchReview = review?.demoId === meta.id ? review : undefined;
  const aiMoments = matchReview?.moments ?? [];
  const selectedSuggestions = aiMoments.filter((_, index) => selectedAi.has(index));
  useEffect(() => { setSelectedAi(new Set()); }, [matchReview?.jobId]);
  const allAiSelected = aiMoments.length > 0 && aiMoments.every((_, index) => selectedAi.has(index));
  const toggleAi = (index: number) => setSelectedAi(current => { const next = new Set(current); next.has(index) ? next.delete(index) : next.add(index); return next; });

  return (
    <Box className="studio-moments">
      <Flex justify="between" gap="3" wrap="wrap" align="center" mb="4"><Box><Heading size="4">{t('studio.yourMoments', { defaultValue: 'Find your next clip' })}</Heading><Text as="p" size="2" color="gray">{t('studio.momentsHint', { defaultValue: 'Choose a player, find a moment, then export it from the original demo.' })}</Text></Box><Badge size="2">{parsed.highlights.length} {t('studio.detectedMoments', { defaultValue: 'detected moments' })}</Badge></Flex>
      <SegmentedControl.Root value={category} onValueChange={value => setState(current => ({ ...current, category: value as MomentCategory }))} className="studio-moment-categories" mb="4" aria-label={t('studio.momentCategory', { defaultValue: 'Moment category' })}><SegmentedControl.Item value="highlights"><i aria-hidden="true" className="bi bi-stars" /> {t('demoView.tabs.highlights')}</SegmentedControl.Item><SegmentedControl.Item value="deaths"><i aria-hidden="true" className="bi bi-heartbreak" /> {t('studio.deaths', { defaultValue: 'Deaths' })}</SegmentedControl.Item><SegmentedControl.Item value="ai"><i aria-hidden="true" className="bi bi-emoji-laughing" /> {t('studio.aiSuggestions', { defaultValue: 'AI suggestions' })}</SegmentedControl.Item><SegmentedControl.Item value="discovery"><i aria-hidden="true" className="bi bi-crosshair" /> Whiffs &amp; shots</SegmentedControl.Item></SegmentedControl.Root>
      {category === 'discovery'&&<DiscoveryMoments meta={meta} parsed={parsed} playerId={playerId} status={status} onSetup={onSetup} onRendered={onRendered}/>}
      {category !== 'ai' && category !== 'discovery' && <>
      <Flex gap="3" align="end" wrap="wrap" mb="3" className="studio-moment-toolbar">
        <Box><Text as="label" htmlFor={`moment-player-${meta.id}`} size="2" mb="1">{t('highlights.col.player')}</Text>
        <Select.Root value={playerFilter} onValueChange={player => setState(current => ({ ...current, player, playerChanged: true }))}>
          <Select.Trigger id={`moment-player-${meta.id}`} className="bounded-select" aria-label={t('highlights.col.player')} title={playerFilter === 'all' ? t('highlights.allPlayers') : displayPlayerName(parsed.stats.find(p => p.steamid === playerFilter)?.name)} />
          <Select.Content>
            <Select.Item value="all">{t('highlights.allPlayers')}</Select.Item>
            {parsed.stats.map((p) => (
              <Select.Item key={p.steamid} value={p.steamid}>
                {displayPlayerName(p.name)} ({p.kills}K/{p.deaths}D)
              </Select.Item>
            ))}
          </Select.Content>
        </Select.Root></Box>
        {category === 'highlights' && <Flex align="center" gap="3" style={{ width: 260, minHeight: 40 }}>
          <Text size="2" style={{ whiteSpace: 'nowrap' }}>
            {t('highlights.minScore', { n: minScore })}
          </Text>
          <Slider aria-label={t('highlights.minScore', { n: minScore })} min={0} max={15} step={1} value={[minScore]} onValueChange={([v]) => setState(current => ({ ...current, minScore: v ?? 0 }))} style={{ flex: 1 }} />
        </Flex>}
        <Box style={{ flex: 1 }} />
        <Text size="2" color="gray">
          {t('highlights.shown', { shown: visible.length, total: parsed.highlights.length })}
        </Text>
        <Button variant="soft" size="2" onClick={() => setSelected(current => { const next = new Set(current); for (const highlight of visible) allVisibleSelected ? next.delete(highlight.id) : next.add(highlight.id); return next; })}>
          {allVisibleSelected ? t('highlights.deselectAll') : t('highlights.selectAll')}
        </Button>
        <Button size="2" disabled={chosen.length === 0} onClick={() => setDialog(true)}>
          <i aria-hidden="true" className="bi bi-camera-video app-icon"  /> {chosen.length ? t('highlights.exportN', { n: chosen.length }) : t('highlights.export')}
        </Button>
      </Flex>
      {playerFilter === 'all' && <Text as="p" size="2" color="gray" mb="3">{playerId && preferredPlayer === 'all' ? t('studio.playerAbsent', { defaultValue: 'Your saved player is not in this match. Choose a player above, or browse all players.' }) : t('studio.choosePersonalPlayer', { defaultValue: 'Choose your player above to see your own gameplay, or browse all players.' })}</Text>}
      {category === 'deaths' && <Text as="p" size="2" color="gray" mb="3">{t('studio.everyDeath', { defaultValue: 'Every detected death is shown here, regardless of highlight score.' })}</Text>}
      {hiddenCount > 0 && <Callout.Root color="amber" mb="3"><Callout.Text>{t('studio.hiddenSelection', { defaultValue: '{{count}} selected moments are hidden by these filters. Export includes them.', count: hiddenCount })}</Callout.Text><Button size="1" variant="soft" onClick={() => setSelected(new Set(visible.filter(highlight => selected.has(highlight.id)).map(highlight => highlight.id)))}>{t('studio.clearHidden', { defaultValue: 'Clear hidden selection' })}</Button></Callout.Root>}

      <Table.Root className="nowrap-headers studio-moments-table" variant="surface" size="2" layout="auto">
        <Table.Header>
          <Table.Row>
            <Table.ColumnHeaderCell width="36px" />
            <Table.ColumnHeaderCell align="right">{t('highlights.col.score')}</Table.ColumnHeaderCell>
            <Table.ColumnHeaderCell align="right">{t('highlights.col.round')}</Table.ColumnHeaderCell>
            <Table.ColumnHeaderCell>{t('highlights.col.time')}</Table.ColumnHeaderCell>
            <Table.ColumnHeaderCell align="right">{t('highlights.col.length')}</Table.ColumnHeaderCell>
            <Table.ColumnHeaderCell>{t('highlights.col.player')}</Table.ColumnHeaderCell>
            <Table.ColumnHeaderCell>{t('highlights.col.what')}</Table.ColumnHeaderCell>
            <Table.ColumnHeaderCell>{t('highlights.col.tags')}</Table.ColumnHeaderCell>
            <Table.ColumnHeaderCell>{t('highlights.details')}</Table.ColumnHeaderCell>
          </Table.Row>
        </Table.Header>
        <Table.Body>
          {visible.map((h) => (
            <Table.Row key={h.id} className={`row-click ${selected.has(h.id) ? 'row-selected' : ''}`} onClick={() => toggle(h.id)}>
              <Table.Cell style={{ verticalAlign: 'middle' }}>
                <Checkbox aria-label={t('studio.selectMoment', { defaultValue: 'Select {{title}}', title: h.title })} style={{ display: 'flex' }} checked={selected.has(h.id)} onCheckedChange={() => toggle(h.id)} onClick={(e) => e.stopPropagation()} />
              </Table.Cell>
              <Table.Cell align="right">
                <Text weight="bold">{h.score.toFixed(1)}</Text>
              </Table.Cell>
              <Table.Cell align="right">{h.round}</Table.Cell>
              <Table.Cell>{clock(h.startTick, tr)}</Table.Cell>
              <Table.Cell align="right">{Math.round((h.endTick - h.startTick) / tr)}s</Table.Cell>
              <Table.Cell>
                <Text truncate style={{ maxWidth: 160, display: 'block' }}>
                  {displayPlayerName(h.player.name)}
                </Text>
              </Table.Cell>
              <Table.Cell style={{ minWidth: 160 }}>{summaryOf(h)}</Table.Cell>
              <Table.Cell>
                <Flex gap="1" wrap="wrap">
                  {h.tags.map((tag) => (
                    <Badge key={tag} size="1" color={HOT_TAGS.has(tag) ? 'amber' : 'gray'} variant={HOT_TAGS.has(tag) ? 'solid' : 'soft'}>
                      {t(TAG_LABELS[tag] ?? `highlights.tags.${tag}`, { defaultValue: tag })}
                    </Badge>
                  ))}
                </Flex>
              </Table.Cell>
              <Table.Cell>
                <Button size="1" variant="soft" onClick={(event) => { event.stopPropagation(); setDetail(h); }}>
                  {t('highlights.details')}
                </Button>
              </Table.Cell>
            </Table.Row>
          ))}
          {visible.length === 0 && (
            <Table.Row>
              <Table.Cell colSpan={9}>
                <Text color="gray">{t('highlights.empty')}</Text>
              </Table.Cell>
            </Table.Row>
          )}
        </Table.Body>
      </Table.Root>
      </>}
      {category === 'ai' && <Box className="studio-ai-moments"><Flex justify="between" align="center" wrap="wrap" gap="3" mb="3"><Box><Heading size="4">{t('studio.possibleBloopers', { defaultValue: 'Possible funny fails & bloopers' })}</Heading><Text as="p" size="2" color="gray">{t('studio.aiEvidenceHint', { defaultValue: 'Local AI suggestions include evidence. Funny moments are subjective; review them before sharing.' })}</Text></Box></Flex>
        <ReviewControls parsed={parsed} player={playerFilter} onPlayer={player=>setState(current=>({...current,player,playerChanged:true}))} review={matchReview} busy={reviewBusy} onReview={onReviewMatch}/>
        {matchReview?.status && <Badge mb="3" color={matchReview.error ? 'red' : 'gray'}>{matchReview.status}</Badge>}{matchReview?.error && <Callout.Root color="red" mb="3"><Callout.Text>{matchReview.error}</Callout.Text></Callout.Root>}
        {aiMoments.length > 0 ? <><Flex gap="3" mb="3" wrap="wrap"><Button variant="soft" onClick={() => setSelectedAi(new Set(allAiSelected ? [] : aiMoments.map((_, index) => index)))}>{allAiSelected ? t('highlights.deselectAll') : t('highlights.selectAll')}</Button><Button disabled={!selectedSuggestions.length} onClick={() => setAiDialog(true)}>{t('highlights.exportN', { n: selectedSuggestions.length })}</Button></Flex>{aiMoments.map((moment, index) => <Box key={`${moment.startTick}:${moment.endTick}:${index}`} className="studio-ai-suggestion"><Flex align="center" gap="3" wrap="wrap"><Checkbox checked={selectedAi.has(index)} onCheckedChange={() => toggleAi(index)} aria-label={t('studio.selectMoment', { defaultValue: 'Select {{title}}', title: moment.title })} /><Text weight="bold">{moment.title}</Text><Badge>{Math.round(moment.confidence)}% {t('studio.confidence', { defaultValue: 'confidence' })}</Badge><Text size="2" color="gray">{clock(moment.startTick, tr)} · {Math.round((moment.endTick - moment.startTick) / tr)}s</Text></Flex><Flex mt="2" gap="1" wrap="wrap">{moment.tags.map(tag => <Badge key={tag} variant="soft" color="gray">{tag}</Badge>)}</Flex><SuggestionEvidence moment={moment} tickRate={tr} /><Flex mt="3"><FeedbackButton tickRate={tr} source={{sourceKind:'moment',demoId:meta.id,momentId:`ai:${moment.startTick}:${moment.endTick}`,playerId:matchReview?.playerId,startTick:moment.startTick,endTick:moment.endTick,originalProposal:{title:moment.title,tags:moment.tags,confidence:moment.confidence,startTick:moment.startTick,endTick:moment.endTick,source:moment.source??'visual-ai'}}}/></Flex></Box>)}</> : <Box className="studio-empty-state" p="5"><i aria-hidden="true" className="bi bi-emoji-laughing studio-empty-icon" /><Heading size="3" mt="2">{t('studio.noSuggestions', { defaultValue: 'No AI suggestions yet' })}</Heading><Text as="p" color="gray" size="2" mt="2">{t('studio.noSuggestionsHint', { defaultValue: 'Start a whole-match review to look for evidence-backed funny moments. A completed review may find none.' })}</Text></Box>}
      </Box>}

      <Dialog.Root open={detail !== null} onOpenChange={(open) => { if (!open) setDetail(null); }}>
        <Dialog.Content maxWidth="640px">
          <Dialog.Title>{t('highlights.scoring.title')}</Dialog.Title>
          <Dialog.Description size="2" mb="3">{detail?.title}</Dialog.Description>
          {detail && <>
            <Text as="p" weight="bold" size="4" mb="2">{t('highlights.scoring.total', { score: detail.score.toFixed(2) })}</Text>
            <Box style={{ maxHeight: '55vh', overflowY: 'auto' }}>
              {SCORE_PARTS.filter((part) => (detail.breakdown[part] ?? 0) > 0).map((part) => (
                <Box key={part} mb="3">
                  <Flex justify="between" gap="3">
                    <Text weight="bold" size="2">{t(`highlights.scoring.${part}.name`)}</Text>
                    <Text weight="bold" size="2" style={{ fontVariantNumeric: 'tabular-nums' }}>+{(detail.breakdown[part] ?? 0).toFixed(2)}</Text>
                  </Flex>
                  <Text as="p" size="2" color="gray">{t(`highlights.scoring.${part}.rule`)}</Text>
                </Box>
              ))}
            </Box>
          </>}
          <Flex justify="end" mt="3" gap="3" wrap="wrap">{detail&&<FeedbackButton tickRate={tr} source={{sourceKind:'moment',demoId:meta.id,momentId:detail.id,playerId:detail.player.steamid,startTick:detail.startTick,endTick:detail.endTick,originalProposal:{title:detail.title,tags:detail.tags,startTick:detail.startTick,endTick:detail.endTick,source:'highlight-detector'}}}/>}<Dialog.Close><Button variant="soft">{t('common.close')}</Button></Dialog.Close></Flex>
        </Dialog.Content>
      </Dialog.Root>

      <ExportDialog open={dialog} onOpenChange={setDialog} count={chosen.length} highlights={chosen} tickRate={tr} seconds={selectedSeconds} status={status} onSetup={onSetup}
        onSubmit={options=>api.render(meta.id,chosen.map(highlight => highlight.id),options)} onSubmitted={()=>{setSelected(new Set());onRendered();}} />
      <ExportDialog open={aiDialog} onOpenChange={setAiDialog} count={selectedSuggestions.length} seconds={selectedSuggestions.reduce((seconds, moment) => seconds + (moment.endTick - moment.startTick) / tr, 0)} status={status} onSetup={onSetup} onSubmit={options => { if (!matchReview) throw new Error('The review is no longer available.'); return automationApi.custom(meta.id, matchReview.playerId, selectedSuggestions.map(moment => [moment.startTick, moment.endTick]), selectedSuggestions.length === 1 ? selectedSuggestions[0]!.title : t('studio.aiClipTitle', { defaultValue: 'AI suggestions' }), { ...options, keyMomentsOnly: false }, 'blooper'); }} onSubmitted={() => { setSelectedAi(new Set()); onRendered(); }} />
    </Box>
  );
}
