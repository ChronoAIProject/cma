export interface ReadSchedule {
  /** Omit for an idle observation that only needs event/focus hints. */
  readonly next?: number;
  readonly cooldown: number;
  /** Explicit refreshes must also respect a failed read's retry delay. */
  readonly failed?: boolean;
}

/** Coalesce read hints without postponing an existing timer or overlapping reads. */
export function observeRead(
  read: (signal: AbortSignal) => Promise<ReadSchedule | undefined>,
  options: { readonly immediateForeground?: boolean } = {},
) {
  const request = new AbortController();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let scheduledAt = Infinity;
  let hintAt = 0;
  let retryAt = 0;
  let reading = false;
  let stopped = false;
  let hinted = false;
  let refreshed = false;
  const hidden = () => document.hidden || document.visibilityState === 'hidden';

  const schedule = (at: number) => {
    if (stopped || request.signal.aborted || hidden()) return;
    if (at >= scheduledAt) return;
    clearTimeout(timer);
    scheduledAt = at;
    timer = setTimeout(() => void run(), Math.max(0, at - Date.now()));
  };
  const run = async () => {
    clearTimeout(timer);
    scheduledAt = Infinity;
    if (stopped || request.signal.aborted || hidden()) return;
    reading = true;
    hinted = false;
    refreshed = false;
    const result = await read(request.signal);
    reading = false;
    if (request.signal.aborted) return;
    if (result === undefined) {
      stopped = true;
      return;
    }
    const now = Date.now();
    hintAt = now + result.cooldown;
    retryAt = result.failed ? hintAt : 0;
    if (result.next !== undefined) schedule(now + result.next);
    if (hinted) schedule(hintAt);
    if (refreshed) schedule(Math.max(now + 100, retryAt));
  };
  const hint = () => {
    if (reading) hinted = true;
    else schedule(Math.max(Date.now(), hintAt));
  };
  const refresh = () => {
    if (reading) refreshed = true;
    else schedule(Math.max(Date.now() + 100, retryAt));
  };
  const foreground = () => {
    if (!options.immediateForeground) return hint();
    if (reading) return;
    if (retryAt > Date.now()) schedule(retryAt);
    else void run();
  };
  const visibility = () => {
    if (hidden()) {
      clearTimeout(timer);
      scheduledAt = Infinity;
    } else foreground();
  };
  document.addEventListener('visibilitychange', visibility);
  window.addEventListener('focus', foreground);
  void run();
  return {
    hint,
    refresh,
    dispose() {
      request.abort();
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', visibility);
      window.removeEventListener('focus', foreground);
    },
  };
}
