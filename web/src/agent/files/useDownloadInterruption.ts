import { useLayoutEffect, useRef } from 'react';

export function useDownloadInterruption(
  reportLocal: (message: string) => void,
  beginDownload?: () => (message: string) => void,
) {
  const route = useRef({ mounted: false, sharedComputerOpen: false });
  useLayoutEffect(() => {
    route.current = { mounted: true, sharedComputerOpen: beginDownload !== undefined };
    return () => {
      route.current.mounted = false;
    };
  }, [beginDownload]);

  return (signal: AbortSignal) => {
    const reportToConsole = beginDownload?.();
    const onAbort = () => {
      const message = 'Download was interrupted. Try downloading the file again.';
      if (reportToConsole && route.current.sharedComputerOpen) reportToConsole(message);
      else if (route.current.mounted) reportLocal(message);
    };
    if (signal.aborted) onAbort();
    else signal.addEventListener('abort', onAbort, { once: true });
    return () => signal.removeEventListener('abort', onAbort);
  };
}
