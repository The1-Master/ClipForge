import { useEffect, useId, useRef, useState, type KeyboardEvent } from 'react';
import { Icon } from './Icon.tsx';

function timeLabel(seconds: number) {
  const value = Math.max(0, Math.floor(Number.isFinite(seconds) ? seconds : 0));
  return `${Math.floor(value / 60)}:${String(value % 60).padStart(2, '0')}`;
}

/** A single mounted player, with native controls available before enhancement. */
export function ClipPlayer({ src, title, poster }: { src: string; title: string; poster?: string }) {
  const helpId = useId();
  const videoRef = useRef<HTMLVideoElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const [playing, setPlaying] = useState(false);
  const [position, setPosition] = useState(0);
  const [duration, setDuration] = useState(0);
  const [muted, setMuted] = useState(false);
  const [volume, setVolume] = useState(1);
  const [error, setError] = useState('');
  const [enhanced, setEnhanced] = useState(false);
  useEffect(() => {
    setEnhanced(true); setError(''); setPlaying(false); setPosition(0); setDuration(0);
    const video = videoRef.current;
    // Restore the source after React's development Strict Mode cleanup.
    if (video && video.getAttribute('src') !== src) { video.setAttribute('src', src); video.load(); }
    if (video) { setMuted(video.muted); setVolume(video.volume); }
    return () => { if (video) { video.pause(); video.removeAttribute('src'); video.load(); } };
  }, [src]);
  const togglePlayback = async () => {
    const video = videoRef.current;
    if (!video) return;
    if (video.paused) {
      try { await video.play(); } catch { setError('Playback could not start. Try again or open the clip from its folder.'); }
    } else video.pause();
  };
  const seek = (value: number) => {
    const video = videoRef.current;
    if (video && Number.isFinite(value)) { video.currentTime = Math.min(duration, Math.max(0, value)); setPosition(video.currentTime); }
  };
  const toggleMute = () => { const video = videoRef.current; if (video) { video.muted = !video.muted; setMuted(video.muted); } };
  const fullscreen = async () => {
    try { if (document.fullscreenElement) await document.exitFullscreen(); else await stageRef.current?.requestFullscreen(); }
    catch { setError('Fullscreen is unavailable in this window.'); }
  };
  const onKeyDown = (event: KeyboardEvent<HTMLVideoElement>) => {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    if (event.key === ' ' || event.key.toLowerCase() === 'k') { event.preventDefault(); void togglePlayback(); }
    else if (event.key === 'ArrowRight') { event.preventDefault(); seek((videoRef.current?.currentTime ?? position) + 5); }
    else if (event.key === 'ArrowLeft') { event.preventDefault(); seek((videoRef.current?.currentTime ?? position) - 5); }
    else if (event.key.toLowerCase() === 'm') { event.preventDefault(); toggleMute(); }
    else if (event.key.toLowerCase() === 'f') { event.preventDefault(); void fullscreen(); }
  };
  return <div className="cf-player" ref={stageRef}>
    <video key={src} ref={videoRef} src={src} poster={poster} controls={!enhanced} preload="metadata" playsInline tabIndex={0}
      aria-label={title} aria-describedby={helpId} onKeyDown={onKeyDown}
      onClick={() => void togglePlayback()} onPlay={() => setPlaying(true)} onPause={() => setPlaying(false)}
      onTimeUpdate={event => setPosition(event.currentTarget.currentTime)}
      onLoadedMetadata={event => setDuration(Number.isFinite(event.currentTarget.duration) ? event.currentTarget.duration : 0)}
      onDurationChange={event => setDuration(Number.isFinite(event.currentTarget.duration) ? event.currentTarget.duration : 0)}
      onVolumeChange={event => { setMuted(event.currentTarget.muted); setVolume(event.currentTarget.volume); }}
      onError={() => setError('This clip could not be loaded. Check that the video still exists in its folder.')} />
    {!playing && !error && <button className="cf-player-bigplay" aria-label={`Play ${title}`} onClick={() => void togglePlayback()}><Icon name="play" size={30} /></button>}
    {enhanced && <div className="cf-player-controls" role="group" aria-label="Video playback controls">
      <button className="cf-control-button" type="button" aria-label={playing ? 'Pause' : 'Play'} onClick={() => void togglePlayback()}><Icon name={playing ? 'pause' : 'play'} /></button>
      <span className="cf-player-time" aria-label={`${timeLabel(position)} of ${timeLabel(duration)}`}>{timeLabel(position)} <span>/ {timeLabel(duration)}</span></span>
      <input className="cf-player-seek" aria-label="Playback position" aria-valuetext={`${timeLabel(position)} of ${timeLabel(duration)}`} type="range" min="0" max={duration || 1} step="0.1" value={Math.min(position, duration || 1)} disabled={!duration} onChange={event => seek(Number(event.currentTarget.value))} />
      <button className="cf-control-button" type="button" aria-label={muted || volume === 0 ? 'Unmute' : 'Mute'} aria-pressed={muted} onClick={toggleMute}><Icon name={muted || volume === 0 ? 'muted' : 'volume'} /></button>
      <input className="cf-player-volume" aria-label="Volume" aria-valuetext={`${Math.round((muted ? 0 : volume) * 100)} percent`} type="range" min="0" max="1" step="0.05" value={muted ? 0 : volume} onChange={event => {
        const video = videoRef.current; if (video) { video.volume = Number(event.currentTarget.value); video.muted = false; setVolume(video.volume); setMuted(false); }
      }} />
      <button className="cf-control-button" type="button" aria-label="Toggle fullscreen" onClick={() => void fullscreen()}><Icon name="fullscreen" /></button>
    </div>}
    <span id={helpId} className="sr-only">Focus the video: Space or K plays and pauses, arrow keys seek five seconds, M mutes, F opens fullscreen.</span>
    {error && <div className="cf-player-error" role="alert"><Icon name="info" /><span>{error}</span></div>}
  </div>;
}
