import { useEffect, useRef } from 'react';
import markMarkup from '../assets/brand/cma-mark.svg?raw';
import './Landing.css';

export type LandingCharacterMode = 'interactive' | 'provisioning';

/** The reserved signed-out character stage; Three.js arrives after the prompt is interactive. */
export function LandingCharacter({
  className,
  mode = 'interactive',
}: {
  readonly className?: string;
  readonly mode?: LandingCharacterMode;
}) {
  const hostRef = useRef<HTMLButtonElement | HTMLDivElement>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (host === null) {
      return;
    }

    let cancelled = false;
    let dispose: (() => void) | undefined;
    const showFallback = () => {
      host.removeAttribute('data-ready');
      if (host instanceof HTMLButtonElement) {
        host.disabled = true;
      }
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
            mode,
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
  }, [mode]);

  const fallback = (
    <span
      className="landing__character-fallback"
      aria-hidden="true"
      dangerouslySetInnerHTML={{ __html: markMarkup }}
    />
  );

  if (mode === 'provisioning') {
    return (
      <div
        ref={(node) => {
          hostRef.current = node;
        }}
        className={`landing__character landing__character--provisioning${className === undefined ? '' : ` ${className}`}`}
        aria-hidden="true"
      >
        {fallback}
      </div>
    );
  }

  return (
    <button
      ref={(node) => {
        hostRef.current = node;
      }}
      className={`landing__character landing__character--interactive${className === undefined ? '' : ` ${className}`}`}
      type="button"
      aria-label="Spin CMA character"
      title="Spin CMA character"
      disabled
    >
      {fallback}
    </button>
  );
}
