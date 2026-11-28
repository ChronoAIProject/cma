import { useEffect, useRef, useState } from 'react';
import { pixelBrand } from '../assets/brand/pixel/identity';
import { useBrandTheme } from '../brand/AgentMascot';
import './LandingCharacter.css';

export type CharacterState = 'brand' | 'running' | 'completed' | 'pending' | 'failed';

export function BrandLoading({ label = 'Loading' }: { readonly label?: string }) {
  return (
    <div className="brand-loading" role="status">
      <LandingCharacter compact state="running" />
      <span>{label}</span>
    </div>
  );
}

/** The reserved signed-out character stage; Three.js arrives after the prompt is interactive. */
export function LandingCharacter({
  state = 'brand',
  compact = false,
  autoRotate = false,
  onActivate,
}: {
  readonly state?: CharacterState;
  readonly compact?: boolean;
  readonly autoRotate?: boolean;
  readonly onActivate?: () => void;
}) {
  const mark = pixelBrand.mark[useBrandTheme()];
  const hostRef = useRef<HTMLButtonElement & HTMLSpanElement>(null);
  const [invited, setInvited] = useState(false);

  useEffect(() => {
    if (onActivate === undefined || invited) return;
    const timer = window.setTimeout(() => setInvited(true), 5_000);
    return () => window.clearTimeout(timer);
  }, [onActivate, invited]);

  useEffect(() => {
    const host = hostRef.current;
    if (host === null) {
      return;
    }

    let cancelled = false;
    let dispose: (() => void) | undefined;
    const showFallback = () => {
      host.removeAttribute('data-ready');
      if (host instanceof HTMLButtonElement) host.disabled = onActivate === undefined;
    };

    if (typeof window.WebGL2RenderingContext === 'undefined') {
      showFallback();
      return;
    }

    void import('./characterScene')
      .then(({ mountCharacter }) => {
        if (cancelled) {
          return;
        }

        try {
          const character = mountCharacter(host, {
            state,
            autoRotate,
            onActivate,
            onReady: () => {
              if (!cancelled) {
                host.setAttribute('data-ready', '');
              }
            },
            onUnavailable: () => {
              if (!cancelled) {
                showFallback();
              }
            },
          });
          dispose = character.dispose;
        } catch {
          showFallback();
        }
      })
      .catch(() => {
        if (!cancelled) {
          showFallback();
        }
      });

    return () => {
      cancelled = true;
      dispose?.();
      showFallback();
    };
  }, [state, autoRotate, onActivate]);

  const Tag = state === 'brand' ? 'button' : 'span';

  return (
    <Tag
      ref={hostRef}
      className={`landing__character${compact ? ' landing__character--compact' : ''}`}
      data-state={state}
      data-invitation={onActivate !== undefined && invited ? '' : undefined}
      data-activation={onActivate !== undefined ? '' : undefined}
      onPointerEnter={onActivate === undefined ? undefined : () => setInvited(true)}
      onFocus={onActivate === undefined ? undefined : () => setInvited(true)}
      type={state === 'brand' ? 'button' : undefined}
      aria-label={
        state === 'brand'
          ? onActivate === undefined
            ? 'Spin CMA character'
            : 'Get Started'
          : undefined
      }
      aria-hidden={state === 'brand' ? undefined : true}
      title={
        state === 'brand'
          ? onActivate === undefined
            ? 'Spin CMA character'
            : 'Get Started'
          : undefined
      }
      disabled={state === 'brand' ? onActivate === undefined : undefined}
      onClick={
        onActivate === undefined
          ? undefined
          : () => {
              if (!hostRef.current?.hasAttribute('data-ready')) onActivate();
            }
      }
    >
      <img className="landing__character-fallback" src={mark} alt="" aria-hidden="true" />
      {onActivate === undefined ? null : (
        <span className="landing__character-invitation" aria-hidden="true">
          <span>Click to start...</span>
        </span>
      )}
    </Tag>
  );
}
