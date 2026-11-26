export function runId(): string {
  return typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `run-${String(Date.now())}-${String(Math.random()).slice(2)}`;
}
