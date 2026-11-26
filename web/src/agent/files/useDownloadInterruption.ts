import { useLayoutEffect, useRef } from 'react';

export function useDownloadInterruption(
  reportLocal: (message: string) => void,
  beginDownload?: () => (message: string) => void,
) {
  const mounted = useRef(false);
  useLayoutEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  return () => {
    const reportAfterUnmount = beginDownload?.();
    return () => {
      const message = 'Download was interrupted. Try downloading the file again.';
      if (mounted.current) reportLocal(message);
      else reportAfterUnmount?.(message);
    };
  };
}
