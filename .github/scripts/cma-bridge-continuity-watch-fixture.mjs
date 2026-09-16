import { ObservationError } from './cma-bridge-continuity-errors.mjs';

export const time = '2026-09-14T00:00:00Z';
export const config = {
  schema: 'cma-bridge-continuity-config-v1', origin: 'https://fixture.example',
  context: 'fixture', namespace: 'disposable', deploymentId: 'continuity',
  tokenFile: '/private/saved-token', agentIds: ['agt_fixture'],
  watch: { durationMs: 1000, intervalMs: 250, sampleTimeoutMs: 250 },
};
export const event = (seq, text = 'private event content') => ({ seq, appended_at: time, kind: { type: 'message', text } });

export function fakeClock() {
  let elapsed = 0;
  let sequence = 0;
  const pending = new Map();
  return {
    monotonic: () => elapsed,
    now: () => new Date(Date.parse(time) + elapsed),
    setTimeout(fn, ms) { const id = ++sequence; pending.set(id, { at: elapsed + ms, fn }); return id; },
    clearTimeout: (id) => pending.delete(id),
    next() { return Math.min(...[...pending.values()].map((entry) => entry.at)); },
    advance(to) {
      elapsed = to;
      for (const [id, entry] of [...pending].sort((a, b) => a[1].at - b[1].at || a[0] - b[0])) {
        if (entry.at <= elapsed && pending.delete(id)) entry.fn();
      }
    },
    pending: () => pending.size,
  };
}

export async function drive(promise, timer) {
  let settled = false;
  promise.then(() => { settled = true; }, () => { settled = true; });
  for (let step = 0; step < 100 && !settled; step += 1) {
    for (let microtask = 0; microtask < 100; microtask += 1) await Promise.resolve();
    if (settled) break;
    const next = timer.next();
    if (!Number.isFinite(next)) throw new Error('fixture has an unresolved operation without a timer');
    timer.advance(next);
  }
  if (!settled) throw new Error('fixture exceeded its deterministic step bound');
  return promise;
}

export function fixture(frames = [{}]) {
  let index = -1;
  const calls = [];
  const state = { aborted: 0, active: 0, maximumActive: 0 };
  const current = () => frames[Math.min(index, frames.length - 1)];
  const stall = (signal) => new Promise((resolve, reject) => {
    state.active += 1;
    state.maximumActive = Math.max(state.maximumActive, state.active);
    const abort = () => { state.active -= 1; state.aborted += 1; reject(signal.reason); };
    signal.addEventListener('abort', abort, { once: true });
  });
  return {
    calls, state,
    get: async (path, authenticated, { signal }) => {
      if (path === '/') index += 1;
      calls.push({ path, authenticated, sample: index });
      if (current().hang === path) return stall(signal);
      if (path === '/') return null;
      if (current().error) throw current().error;
      if (path.includes('/events?')) {
        const after = Number(new URL(path, config.origin).searchParams.get('after'));
        return current().pages?.[after] ?? { items: current().events ?? [event(1)] };
      }
      return {
        id: 'agt_fixture', sandbox: { id: 'sbx_fixture' }, workspace: { id: current().workspace ?? 'wks_fixture' },
        worktree: { id: 'wkt_fixture' }, profile: { id: 'agp_fixture', revision: 1 }, created_at: time, status: 'ready',
      };
    },
    pods: async ({ signal }) => {
      calls.push({ kind: 'pods', sample: index });
      if (current().hang === 'pods') return stall(signal);
      return { items: [{
        metadata: { name: 'retained-pod', uid: current().uid ?? 'retained-uid', labels: { 'cma/sandbox': 'sbx_fixture', 'cma/runtime-generation': '1', 'cma/runtime-contract': '3' } },
        spec: { containers: [{ name: 'runtime' }] },
        status: { phase: 'Running', containerStatuses: [{ name: 'runtime', ready: true, imageID: `registry/runtime@sha256:${'a'.repeat(64)}`, containerID: 'containerd://retained', restartCount: current().restarts ?? 0 }] },
      }] };
    },
  };
}

export const unavailable = () => new ObservationError('public_read_failed_503');
