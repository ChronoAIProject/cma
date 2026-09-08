import { useEffect, useRef } from 'react';
import markMarkup from '../assets/brand/cma-mark.svg?raw';
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
}: {
  readonly state?: CharacterState;
  readonly compact?: boolean;
  readonly autoRotate?: boolean;
}) {
  const hostRef = useRef<HTMLButtonElement & HTMLSpanElement>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (host === null) {
      return;
    }

    let cancelled = false;
    let dispose: (() => void) | undefined;
    const showFallback = () => {
      host.removeAttribute('data-ready');
      if (host instanceof HTMLButtonElement) host.disabled = true;
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
  }, [state, autoRotate]);

  const Tag = state === 'brand' ? 'button' : 'span';

  return (
    <Tag
      ref={hostRef}
      className={`landing__character${compact ? ' landing__character--compact' : ''}`}
      data-state={state}
      type={state === 'brand' ? 'button' : undefined}
      aria-label={state === 'brand' ? 'Spin CMA character' : undefined}
      aria-hidden={state === 'brand' ? undefined : true}
      title={state === 'brand' ? 'Spin CMA character' : undefined}
      disabled={state === 'brand' ? true : undefined}
    >
      <span
        className="landing__character-fallback"
        aria-hidden="true"
        dangerouslySetInnerHTML={{ __html: markMarkup }}
      />
    </Tag>
  );
}
