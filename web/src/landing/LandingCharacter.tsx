import { useEffect, useRef } from 'react';
import markMarkup from '../assets/brand/cma-mark.svg?raw';

/** The reserved signed-out character stage; Three.js arrives after the prompt is interactive. */
export function LandingCharacter() {
  const hostRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (host === null) {
      return;
    }

    let cancelled = false;
    let dispose: (() => void) | undefined;
    const showFallback = () => {
      host.removeAttribute('data-ready');
      host.disabled = true;
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
  }, []);

  return (
    <button
      ref={hostRef}
      className="landing__character"
      type="button"
      aria-label="Spin CMA character"
      title="Spin CMA character"
      disabled
    >
      <span
        className="landing__character-fallback"
        aria-hidden="true"
        dangerouslySetInnerHTML={{ __html: markMarkup }}
      />
    </button>
  );
}
