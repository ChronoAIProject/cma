import { useEffect, useRef, useState, type RefObject } from 'react';

export interface ComputerFullscreenFallback {
  readonly computerExpanded: boolean;
  readonly setComputerExpanded: (expanded: boolean) => void;
}

export function useComputerFullscreen(
  computerRef: RefObject<HTMLElement | null>,
  fullscreenFallback?: ComputerFullscreenFallback,
) {
  const [fullScreen, setFullScreen] = useState(false);
  const [screenError, setScreenError] = useState<string>();
  const enteringFullScreen = useRef(false);
  const expanded = fullscreenFallback?.computerExpanded === true;
  const exitFullScreen = () => {
    void document.exitFullscreen().catch(() => setScreenError('Press Esc to exit full screen.'));
  };
  const toggleScreenSize = () => {
    const computer = computerRef.current;
    if (!computer || enteringFullScreen.current) return;
    setScreenError(undefined);
    if (expanded) {
      fullscreenFallback?.setComputerExpanded(false);
      return;
    }
    if (document.fullscreenElement === computer) {
      exitFullScreen();
      return;
    }
    const expand = (reason: string) => {
      if (
        computerRef.current !== computer ||
        computer.hidden ||
        document.fullscreenElement === computer
      )
        return;
      fullscreenFallback?.setComputerExpanded(true);
      setScreenError(reason);
    };
    if (typeof computer.requestFullscreen !== 'function' || document.fullscreenEnabled === false) {
      expand(
        'Full screen is unavailable here. To use it, open CMA in a tab that allows full screen.',
      );
    } else {
      enteringFullScreen.current = true;
      void computer
        .requestFullscreen()
        .catch(() => {
          expand(
            'The browser declined full screen. Allow full screen in your browser and try again.',
          );
        })
        .finally(() => {
          enteringFullScreen.current = false;
        });
    }
  };
  const screenSizeLabel = expanded
    ? 'Exit expanded view'
    : fullScreen
      ? 'Exit full screen'
      : 'Full screen';
  useEffect(() => {
    const changed = () => {
      setFullScreen(document.fullscreenElement === computerRef.current);
      setScreenError(undefined);
    };
    document.addEventListener('fullscreenchange', changed);
    return () => document.removeEventListener('fullscreenchange', changed);
  }, [computerRef]);
  return {
    fullScreen,
    expanded,
    screenError,
    clearScreenError: () => setScreenError(undefined),
    exitFullScreen,
    toggleScreenSize,
    screenSizeLabel,
  };
}
