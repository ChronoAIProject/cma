import { useEffect } from 'react';

export const unconfirmedSettingsWarning =
  'This update is not yet confirmed. Retry or wait here before switching Agents or leaving. Leaving loses the local retry details and does not cancel an update the server may have accepted.';

export function useUnconfirmedSettings(unconfirmed: boolean) {
  useEffect(() => {
    if (!unconfirmed) return;
    const warn = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = '';
    };
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [unconfirmed]);
}
