import { useEffect, useState } from 'react';

/** Countdown is presentation only; the read owner retains the actual deadline. */
export function RetryNotice({ at }: { readonly at: number }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = setInterval(() => {
      if (!document.hidden) setNow(Date.now());
    }, 1000);
    return () => clearInterval(timer);
  }, [at]);
  return <p role="status">Busy, retrying in {Math.max(0, Math.ceil((at - now) / 1000))}s…</p>;
}
