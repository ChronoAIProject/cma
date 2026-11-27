import { type Ref, useImperativeHandle, useRef, useState, useSyncExternalStore } from 'react';
import { agentMascotAssets } from './agentMascotAssets';
import type { AgentVisual } from './agentVisual';
import { useMascotFrame, type MascotPlayback } from './useMascotFrame';
import './agent-mascot.css';

export type AgentMascotSize = 16 | 32 | 48 | 96;

interface AgentMascotProps {
  readonly visual: Pick<AgentVisual, 'action' | 'label'>;
  readonly size: AgentMascotSize;
  readonly ref?: Ref<HTMLSpanElement>;
  readonly playback?: MascotPlayback;
  readonly decorative?: boolean;
}

function resolvedTheme() {
  return document.body.hasAttribute('data-ds-dark-theme') ? 'dark' : 'light';
}

function subscribeToTheme(notify: () => void) {
  const observer = new MutationObserver(notify);
  observer.observe(document.body, { attributeFilter: ['data-ds-dark-theme'] });
  return () => observer.disconnect();
}

export function useBrandTheme() {
  return useSyncExternalStore(subscribeToTheme, resolvedTheme);
}

function Poster({ src, size }: { readonly src: string; readonly size: AgentMascotSize }) {
  const [failed, setFailed] = useState(false);
  return failed ? (
    <span className="agent-mascot__fallback" />
  ) : (
    <img
      className="agent-mascot__poster"
      src={src}
      alt=""
      width={size}
      height={size}
      onError={() => setFailed(true)}
    />
  );
}

function Sprite({
  src,
  poster,
  size,
  frame,
  frames,
}: {
  readonly src: string;
  readonly poster: string;
  readonly size: AgentMascotSize;
  readonly frame: number;
  readonly frames: number;
}) {
  const [loaded, setLoaded] = useState(false);
  return (
    <span className="agent-mascot__sprite" style={{ inlineSize: size, blockSize: size }}>
      {loaded ? null : <Poster src={poster} size={size} />}
      <img
        hidden={!loaded}
        src={src}
        alt=""
        width={size * frames}
        height={size}
        onLoad={() => setLoaded(true)}
        onError={() => setLoaded(false)}
        style={{ transform: `translateX(${-frame * size}px)` }}
      />
    </span>
  );
}

export function AgentMascot({ visual, size, ref, playback, decorative }: AgentMascotProps) {
  const theme = useBrandTheme();
  const slot = useRef<HTMLSpanElement>(null);
  useImperativeHandle(ref, () => slot.current!, []);
  const motion = agentMascotAssets[visual.action];
  const { posters, sprite } = motion[theme];
  const frame = useMascotFrame(slot, visual.action, motion.frameDurationsMs, playback);
  const poster = posters[size === 96 ? 48 : size];
  const slotSize = size === 16 ? 24 : size;

  return (
    <span className="agent-mascot">
      <span
        ref={slot}
        className="agent-mascot__artwork"
        style={{ inlineSize: slotSize, blockSize: slotSize }}
        aria-hidden="true"
      >
        {frame === undefined ? (
          <Poster key={poster} src={poster} size={size} />
        ) : (
          <Sprite
            key={sprite}
            src={sprite}
            poster={poster}
            size={size}
            frame={frame}
            frames={motion.frameDurationsMs.length}
          />
        )}
      </span>
      {decorative ? null : <span>{visual.label}</span>}
    </span>
  );
}
