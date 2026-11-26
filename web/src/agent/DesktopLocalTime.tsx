import { useEffect, useState } from 'react';

export function DesktopLocalTime() {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const refresh = () => {
      clearTimeout(timer);
      if (document.visibilityState === 'hidden') return;
      const current = Date.now();
      setNow(current);
      timer = setTimeout(refresh, 60_000 - (current % 60_000));
    };
    refresh();
    document.addEventListener('visibilitychange', refresh);
    return () => {
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', refresh);
    };
  }, []);
  // Resolve the device timezone again on refresh so an open panel follows changes.
  const formatter = new Intl.DateTimeFormat(undefined, {
    weekday: 'short',
    hour: '2-digit',
    minute: '2-digit',
  });
  return (
    <p title="Your device’s local time. The desktop clock follows the sandbox timezone.">
      Local time · {formatter.resolvedOptions().timeZone} ·{' '}
      <time dateTime={new Date(now).toISOString()}>{formatter.format(now)}</time>
    </p>
  );
}
