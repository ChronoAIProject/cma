import { performance } from 'node:perf_hooks';
import { createHash } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';

export const clock = () => performance.now();
export const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
export const hash = (value) => createHash('sha256').update(value).digest('hex');
export const cases = {
  create_plain: { label: '1: Agent + plain Sandbox ready', limit: 5000 },
  create_desktop: { label: '1: Agent + desktop Sandbox ready', limit: 5000 },
  agent_open: { label: '2: Agent rendered and interactive', limit: 1000 },
  message_send: { label: '3: Message accepted and shown', limit: 1000 },
  message_stop: { label: '3: Stop acknowledged and reflected', limit: 1000 },
  admin_report: { label: '4: Admin Sandbox usage rendered', limit: 2000 },
};

export class CheckError extends Error {
  constructor(code, status, problemCode) { super(code); this.code = code; this.status = status; this.problemCode = problemCode; }
}
export function requireCheck(value, code, status, problemCode) {
  if (!value) throw new CheckError(code, status, problemCode);
}
export const safeFailure = (error) => ({
  code: error instanceof CheckError ? error.code : error?.name === 'TimeoutError' ? 'timeout' : 'check_failed',
  ...(Number.isInteger(error?.status) ? { status: error.status } : {}),
  ...(typeof error?.problemCode === 'string' && /^[a-z][a-z0-9_]{1,63}$/.test(error.problemCode) ? { problem_code: error.problemCode } : {}),
});

export async function poll(read, accept, { timeout = 120000, interval = 250 } = {}) {
  const started = clock();
  for (;;) {
    const value = await read(Math.max(1, timeout - (clock() - started)));
    if (accept(value)) return value;
    if (clock() - started >= timeout) throw new CheckError('completion_timeout');
    await delay(Math.min(interval, timeout - (clock() - started)));
  }
}

export function stats(rows, limit) {
  const completed = rows.filter((row) => row.outcome === 'complete');
  const values = completed.map((row) => row.elapsed_ms).sort((a, b) => a - b);
  const at = (p) => values.length ? values[Math.max(0, Math.ceil(p * values.length) - 1)] : null;
  return {
    attempts: rows.filter((row) => row.outcome !== 'blocked').length,
    completed: values.length,
    failed: rows.filter((row) => row.outcome === 'failed').length,
    blocked: rows.filter((row) => row.outcome === 'blocked').length,
    p50_ms: at(0.5), p95_ms: at(0.95), max_ms: at(1),
    threshold_ms: limit ?? null,
    breaches: limit && values.length ? values.filter((value) => value > limit).length : null,
    acceptance: limit == null ? 'diagnostic_only'
      : rows.length < 10 || values.length !== rows.length ? 'NOT_PROVEN'
      : values.at(-1) <= limit ? 'PASS' : 'FAIL',
  };
}

export class Results {
  constructor(meta) { this.meta = meta; this.samples = []; }
  async measure(name, run, action, observer) {
    const start = clock();
    const utcMs = Date.now();
    let outcome = 'complete'; let evidence;
    try {
      observer?.startAction(name, run, { monotonicMs: start, utcMs });
      evidence = await action();
    } catch (error) {
      outcome = 'failed'; evidence = safeFailure(error);
    }
    const elapsedMs = clock() - start;
    this.samples.push({ case: name, run, outcome, started_at: new Date(utcMs).toISOString(), action_started_monotonic_ms: start, elapsed_ms: elapsedMs, ...evidence });
    try { observer?.endAction({ elapsedMs, outcome, ...(outcome === 'failed' ? { code: evidence.code } : {}) }); }
    catch { throw new CheckError('phase_capture_failed'); }
    return outcome === 'complete';
  }
  block(name, reason, run = null) {
    this.samples.push({ case: name, run, outcome: 'blocked', code: reason });
  }
  async save(directory) {
    await mkdir(directory, { recursive: true });
    const summary = Object.fromEntries([...new Set([...Object.keys(cases), ...this.samples.map((x) => x.case)])]
      .map((name) => [name, stats(this.samples.filter((x) => x.case === name), cases[name]?.limit)]));
    // A synthetic run or unconfirmed revision can never establish production acceptance.
    for (const [name, row] of Object.entries(summary)) {
      if (cases[name] && this.meta.environment !== 'production') row.acceptance = 'NOT_PROVEN';
      else if (cases[name] && !this.meta.revision_confirmed && row.acceptance === 'PASS') row.acceptance = 'NOT_PROVEN';
    }
    const report = { metadata: this.meta, summary, samples: this.samples };
    await writeFile(`${directory}/results.json`, JSON.stringify(report, null, 2) + '\n', { mode: 0o600 });
    const fmt = (n) => n == null ? '—' : n.toFixed(1);
    const lines = [
      `CMA client timing: ${this.meta.environment} / ${this.meta.mode}`,
      `Started: ${this.meta.started_at}. Origin: ${this.meta.origin}.`,
      `Revision confirmed: ${this.meta.revision_confirmed ? 'yes' : 'no'}.`,
      '', '| Case | Attempts | Complete | Failed / blocked | p50 ms | p95 ms | max ms | Limit ms | Breaches | Acceptance |',
      '|---|---:|---:|---:|---:|---:|---:|---:|---:|---|',
      ...Object.entries(summary).map(([name, r]) => `| ${cases[name]?.label ?? name} | ${r.attempts} | ${r.completed} | ${r.failed} / ${r.blocked} | ${fmt(r.p50_ms)} | ${fmt(r.p95_ms)} | ${fmt(r.max_ms)} | ${r.threshold_ms ?? '—'} | ${r.breaches ?? '—'} | ${r.acceptance} |`),
      '', 'Percentiles use nearest rank over completed attempts. Failed attempts retain elapsed time in JSON and prevent a pass. At n=10, p95 is the maximum. All attempts are serial; no discarded warmups.',
      'Public probes and local fixtures are diagnostic only. A signed-out shell, admission receipt alone, optimistic text, or an unrendered report is not owner acceptance.',
      'Browser timing starts before navigation/click and includes client scheduling, auth, network, polling and rendering. HTTP timing includes complete response bodies. Chrome routing disables HTTP cache; runs use fresh pages.',
      'No response bodies, credentials, raw headers, screenshots, traces, browser storage or personal identifiers are written to timing artifacts.',
      ...(this.meta.notes ?? []).map((note) => `\n${note}`), '',
    ];
    await writeFile(`${directory}/summary.md`, lines.join('\n'), { mode: 0o600 });
    return report;
  }
}

export async function publicRead(origin, path, timeout = 30000) {
  const url = new URL(path, origin);
  requireCheck(url.origin === origin && !url.username && !url.password, 'origin_refused');
  const response = await fetch(url, { redirect: 'manual', signal: AbortSignal.timeout(timeout) });
  const body = await response.text();
  return { status: response.status, body, contentType: response.headers.get('content-type') ?? '' };
}

export function agentReady(agent) {
  return agent?.status === 'ready' && agent?.sandbox?.status === 'active';
}
