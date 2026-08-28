import { useEffect, useState } from 'react';

/** Keep an action blocked until the current server-owned retry deadline elapses. */
export function useRetryDeadline(deadline: number | null): boolean {
  const [releasedDeadline, setReleasedDeadline] = useState<number | null>(null);

  useEffect(() => {
    if (deadline === null) return;
    const timer = window.setTimeout(
      () => setReleasedDeadline(deadline),
      Math.max(0, deadline - Date.now()),
    );
    return () => window.clearTimeout(timer);
  }, [deadline]);

  return deadline !== null && releasedDeadline !== deadline;
}
