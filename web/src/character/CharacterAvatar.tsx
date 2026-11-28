import { useContext, useEffect, useRef, useState } from 'react';
import { pixelBrand } from '../assets/brand/pixel/identity';
import { useBrandTheme } from '../brand/AgentMascot';
import { CharacterColorContext } from './characterColor';
import type { CharacterPose } from './poses';
import type { CharacterHandle, CharacterOptions, CharacterTone } from './stage';
import './CharacterAvatar.css';

interface CharacterAvatarProps {
  readonly pose: CharacterPose;
  /** Rendered square edge in CSS pixels. */
  readonly size: number;
  readonly still?: boolean;
  readonly shadow?: boolean;
  readonly tone?: CharacterTone;
  /**
   * The Sandbox's colour as `#rrggbb`; without it the surrounding `CharacterColorContext`
   * applies. It replaces only the brand body: approval and failure keep their status colour.
   */
  readonly color?: string;
  /** Seconds to offset this character's clock from its neighbours. */
  readonly phase?: number;
  readonly className?: string;
}

/** A stable phase in [0, 7) seconds from any identifier, so a list never moves in lockstep. */
export function characterPhase(id: string): number {
  let hash = 0;
  for (let index = 0; index < id.length; index += 1) {
    hash = (hash * 31 + id.charCodeAt(index)) >>> 0;
  }
  return (hash % 700) / 100;
}

/**
 * The shipped 3D character as a decorative avatar. It reserves its square at once, draws
 * through the page's one shared stage, and falls back to the pixel brand mark when WebGL is
 * unavailable. It is always `aria-hidden`: the caller keeps the status in words.
 */
export function CharacterAvatar({
  pose,
  size,
  still = false,
  shadow = false,
  tone = 'brand',
  color,
  phase = 0,
  className,
}: CharacterAvatarProps) {
  const mark = pixelBrand.mark[useBrandTheme()];
  const inherited = useContext(CharacterColorContext);
  const body = tone === 'brand' ? (color ?? inherited) : undefined;
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const handleRef = useRef<CharacterHandle | undefined>(undefined);
  const [status, setStatus] = useState<'loading' | 'ready' | 'unavailable'>('loading');
  const optionsRef = useRef<CharacterOptions>({ pose, still, shadow, tone, color: body, phase });

  // Declared before the mount effect so a stage that resolves later attaches the latest options.
  useEffect(() => {
    const next = { pose, still, shadow, tone, color: body, phase };
    optionsRef.current = next;
    handleRef.current?.update(next);
  }, [pose, still, shadow, tone, body, phase]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (canvas === null) return;
    let cancelled = false;
    void import('./stageBrowser')
      .then(({ characterStage }) => {
        if (cancelled) return;
        const handle = characterStage.attach(canvas, optionsRef.current, () => {
          if (!cancelled) setStatus('unavailable');
        });
        if (handle === undefined) {
          setStatus('unavailable');
          return;
        }
        handleRef.current = handle;
        setStatus('ready');
      })
      .catch((error: unknown) => {
        console.error('[character] stage failed to load; showing the still mark', error);
        if (!cancelled) setStatus('unavailable');
      });
    return () => {
      cancelled = true;
      handleRef.current?.dispose();
      handleRef.current = undefined;
    };
  }, []);

  return (
    <span
      className={className === undefined ? 'character-avatar' : `character-avatar ${className}`}
      style={{ width: size, height: size, color: body }}
      data-pose={pose}
      data-tone={tone}
      data-motion={still ? 'still' : 'live'}
      data-status={status}
      aria-hidden="true"
    >
      <canvas ref={canvasRef} className="character-avatar__canvas" />
      {status === 'unavailable' ? (
        <img className="character-avatar__fallback" src={mark} alt="" />
      ) : null}
    </span>
  );
}
