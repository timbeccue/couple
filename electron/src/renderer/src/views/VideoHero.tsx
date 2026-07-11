// VideoHero.tsx — the muted video slaved to the audition audio clock (§11.4,
// A/V skew ≤ 1 frame): the Electron stand-in for the mac shell's muted
// AVPlayer. An rAF loop measures skew = video.currentTime − clock(); a large
// skew hard-seeks, a small one micro-slews playbackRate (±5%). The HUD
// (COUPLE_HUD=1) surfaces measured skew + pump underruns — the budget is
// verified, not assumed.
import React, { useEffect, useRef, useState } from 'react';
import { appModel } from '../app/AppModel';
import type { CoupleRow } from '../app/CoupleRow';
import { useModel } from '../app/observable';
import { env } from '../kit/native';
import { mediaUrl } from '../app/mediaUrl';
import type { MonitorMode } from '../kit/Audition';

const HARD_SEEK_S = 0.25;
const SLEW_GAIN = 1.0; // rate delta per second of skew…
const SLEW_MAX = 0.05; // …clamped to ±5%
const ANCHOR_COLORS = ['#a68cfa', '#fa7085', '#61a6fa', '#a3e636'];

function timecode(t: number): string {
  const s = Math.max(0, t);
  return `${Math.floor(s / 60)}:${(s % 60).toFixed(2).padStart(5, '0')}`;
}

export function VideoHero({ couple }: { couple: CoupleRow }): React.JSX.Element {
  const audition = useModel(appModel.audition);
  const pair = useModel(couple.pair);
  const videoRef = useRef<HTMLVideoElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const lastLog = useRef(0);
  const [hud, setHud] = useState<{ skewMs: number; underruns: number } | null>(null);
  const [videoError, setVideoError] = useState<string | null>(null);

  const isAttachedHere = audition.couple === couple;
  const playing = isAttachedHere && audition.playing;
  const clock = audition.clock();
  const headT = Number.isFinite(clock)
    ? clock
    : isAttachedHere && audition.lastSeekT !== null
      ? audition.lastSeekT
      : pair.playheadT;

  // Paused frame tracking: the audio clock is the master while playing; when
  // paused, the displayed frame follows the playhead/seek position instead.
  useEffect(() => {
    const video = videoRef.current;
    if (!playing && video && Number.isFinite(headT) && Math.abs(video.currentTime - headT) > 0.02) {
      video.currentTime = headT;
    }
  }, [playing, headT]);

  const jumpToAnchor = (i: number, t: number): void => {
    if (!isAttachedHere) appModel.attachAudition(couple);
    pair.focusedAnchor = i;
    appModel.audition.seek(Math.max(0, t - 0.8)); // pre-roll (§EditorComponents)
    appModel.audition.play();
    pair.invalidate();
  };

  const toggleFullscreen = (): void => {
    const el = containerRef.current;
    if (!el) return;
    if (document.fullscreenElement) void document.exitFullscreen();
    else void el.requestFullscreen?.();
  };

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    if (!playing) {
      video.pause();
      video.playbackRate = 1;
      return;
    }
    let raf = 0;
    void video.play().catch(() => {});
    const tick = () => {
      const clock = appModel.audition.clock();
      if (Number.isFinite(clock)) {
        const skew = video.currentTime - clock;
        if (Math.abs(skew) > HARD_SEEK_S) {
          video.currentTime = clock;
          video.playbackRate = 1;
        } else {
          video.playbackRate = 1 - Math.max(-SLEW_MAX, Math.min(SLEW_MAX, skew * SLEW_GAIN));
        }
        if (env.COUPLE_HUD) {
          setHud({ skewMs: skew * 1000, underruns: appModel.audition.underruns() });
          const now = performance.now();
          if (now - lastLog.current > 1000) {
            lastLog.current = now;
            // Greppable in harness runs: the measured A/V budget (§11.4).
            console.log(
              `[hud] skew_ms=${(skew * 1000).toFixed(1)} underruns=${appModel.audition.underruns()} v=${video.currentTime.toFixed(3)} c=${clock.toFixed(3)} rate=${video.playbackRate.toFixed(3)} ready=${video.readyState}`
            );
          }
        }
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(raf);
      video.pause();
      video.playbackRate = 1;
    };
  }, [playing]);

  return (
    <div className="video-hero-wrap">
      <div className="video-hero" ref={containerRef}>
        <video
          ref={videoRef}
          muted
          playsInline
          src={mediaUrl(couple.video.path)}
          onClick={() => {
            if (!isAttachedHere) appModel.attachAudition(couple);
            appModel.audition.toggle();
          }}
          onError={(e) => {
            const mediaErr = (e.currentTarget as HTMLVideoElement).error;
            const codec = couple.video.info?.video?.codec ?? 'unknown';
            console.error(
              `[video] element error code=${mediaErr?.code} msg=${mediaErr?.message ?? ''} codec=${codec} path=${couple.video.path}`
            );
            setVideoError(codec);
          }}
          onCanPlay={() => setVideoError(null)}
        />
        {videoError !== null && (
          <div className="video-error">
            This shell can't decode the video stream ({videoError}) — Chromium has no decoder
            for it. Audio sync still works; the mac app plays it via AVFoundation.
          </div>
        )}
        {hud && (
          <div className="av-hud">
            skew {hud.skewMs.toFixed(1)} ms · underruns {hud.underruns}
          </div>
        )}
      </div>
      <div className="hero-transport">
        <button
          className="hero-btn"
          disabled={!couple.hasSession}
          onClick={() => {
            if (!isAttachedHere) appModel.attachAudition(couple);
            appModel.audition.toggle();
          }}
        >
          {playing ? '❚❚' : '▶'}
        </button>
        <span className={`hero-time${playing ? ' live' : ''}`}>{timecode(headT)}</span>
        <span className="wave-spacer" />
        {pair.anchors.map((a, i) => (
          <button
            key={i}
            className="hero-anchor"
            style={{
              background: `${ANCHOR_COLORS[i % 4]}${pair.focusedAnchor === i ? 'e6' : '73'}`,
            }}
            title={`Play from anchor ${i + 1} (−0.8 s)`}
            onClick={() => jumpToAnchor(i, a.t)}
          >
            {i + 1}
          </button>
        ))}
        <div className="seg">
          {(['pan', 'overlay', 'null'] as MonitorMode[]).map((m) => (
            <button
              key={m}
              className={audition.mode === m ? 'on' : undefined}
              disabled={!couple.hasSession}
              onClick={() => audition.setMode(m)}
            >
              {m === 'pan' ? 'Pan' : m === 'overlay' ? 'Overlay' : 'Null'}
            </button>
          ))}
        </div>
        <button className="hero-btn" title="Fullscreen video" onClick={toggleFullscreen}>
          ⤢
        </button>
      </div>
    </div>
  );
}
