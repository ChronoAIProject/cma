/** Bounded readiness fallback; event hints never authorize an action themselves. */
export class ReadinessWake {
  private readonly listeners = new Set<() => void>();
  hint() {
    for (const listener of this.listeners) listener();
  }
  async wait(ms: number, signal: AbortSignal) {
    signal.throwIfAborted();
    await new Promise<void>((resolve, reject) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const started = Date.now();
      let due = started + ms;
      const finish = () => {
        clearTimeout(timer);
        signal.removeEventListener('abort', abort);
        document.removeEventListener('visibilitychange', visible);
        this.listeners.delete(hint);
        if (signal.aborted) reject(signal.reason);
        else resolve();
      };
      const ready = () => {
        due = Infinity;
        if (!document.hidden) finish();
      };
      const hint = () => {
        if (document.hidden) return;
        const at = Math.max(Date.now(), started + 1000);
        if (at >= due) return;
        due = at;
        clearTimeout(timer);
        // A burst of frames cannot create a faster-than-once-per-second loop.
        timer = setTimeout(ready, Math.max(0, at - Date.now()));
      };
      const visible = () => {
        if (!document.hidden) hint();
      };
      const abort = () => finish();
      signal.addEventListener('abort', abort, { once: true });
      document.addEventListener('visibilitychange', visible);
      this.listeners.add(hint);
      timer = setTimeout(ready, ms);
    });
  }
}

export function readinessDelay(attempt: number, base = 1000) {
  return Math.min(base * 2 ** Math.min(attempt, 3), 5000) * (0.9 + Math.random() * 0.2);
}
