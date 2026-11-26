import { check, clock, hash, safeFailure, GateError } from '../lib/checks.mjs';
import { CLEANUP_MS, SETTLE_MS, SETUP_MS, validateAuthority } from './authority.mjs';
import { Fixture } from './resources.mjs';
import { Turn } from './turn.mjs';

// Internal run owner. The adapter exposes only its frozen domain facade.
export class StagingRun {
  constructor(config, { wire, readAuthority, connect, evidence, time = clock }) {
    Object.assign(this, { config, wire, readAuthority, connect, evidence, time });
    this.fixtures = new Map(); this.connections = new Map(); this.pending = new Set(); this.rates = new Map();
    this.offered = 0; this.stopped = false; this.preparing = false;
    this.observation = new AbortController();
  }
  record(row) {
    try { return this.evidence.record(row); }
    catch {
      this.stopped = true; this.evidenceFailed = true;
      this.observation.abort();
      if (!this.closing) throw new GateError('evidence_write_failed');
      // Custody stays in memory and cleanup must proceed even when its evidence sink fails.
      return row;
    }
  }
  stop(code = 'operator_stop') {
    this.observation.abort();
    if (this.stopped) return;
    this.stopped = true;
    try { this.record({ scenario: 'stop', status: 'BLOCKED', code }); }
    catch { /* Admission is already latched closed; finalization still owns every fixture. */ }
  }
  remaining(deadline) {
    const remaining = deadline - this.time.now();
    check(remaining > 0, 'operation_deadline'); return Math.max(1, Math.min(remaining, 30000));
  }
  async authority(cleanup = false) {
    return validateAuthority(this.config, await this.readAuthority(), { cleanup });
  }
  async quota(spec, kind, deadline) {
    const group = kind === 'sandbox' ? 'create' : kind;
    const key = `${spec.subject}:${group}`;
    const limit = group === 'read' ? Math.floor(spec.readPerMinute * 0.8) : Math.min(8, Math.floor(spec.createPerMinute * 0.8));
    const ledgers = [[key, limit], ...(kind === 'sandbox' ? [['sandbox_global', 4]] : [])];
    for (;;) {
      const now = this.time.now(); let wait = 0;
      for (const [id, cap] of ledgers) {
        const rows = (this.rates.get(id) ?? []).filter((t) => now - t < 60000);
        this.rates.set(id, rows);
        if (rows.length >= cap) wait = Math.max(wait, 60000 - (now - rows[0]));
      }
      if (!wait) { for (const [id] of ledgers) this.rates.get(id).push(now); return; }
      await this.time.sleep(Math.min(wait, this.remaining(deadline)));
    }
  }
  async admit(spec, kind, deadline) {
    check(!this.stopped, 'admission_stopped');
    await this.authority();
    if (kind !== 'turn') await this.quota(spec, kind, deadline);
    const receipt = await this.authority();
    check(!this.stopped, 'admission_stopped'); this.remaining(deadline);
    if (kind === 'turn') {
      check(this.offered < this.config.maxTurns, 'turn_budget_exhausted');
      // Retain every local maximum liability until the run ends, even after settlement.
      check(receipt.spentAndReserved + (this.offered + 1) * this.config.maxTurnLiability <= this.config.modelCap,
        'model_budget_exhausted');
    }
    if (kind === 'turn') this.offered++;
  }
  track(action) {
    const task = action().catch((error) => { this.stop('operation_failed'); throw error; });
    this.pending.add(task); task.then(() => this.pending.delete(task), () => this.pending.delete(task));
    return task;
  }
  create(index) {
    return this.track(async () => {
      check(!this.stopped && !this.preparing, 'setup_stopped_or_in_progress');
      const spec = this.config.fixtures.find((f) => f.index === index);
      check(spec && !this.fixtures.has(index), 'fixture_not_authorized_or_reused');
      this.preparing = true;
      try {
        await this.authority();
        let http = this.connections.get(spec.subject);
        if (!http) { http = await this.connect(spec); this.connections.set(spec.subject, http); }
        const fixture = new Fixture(this, spec, http);
        this.fixtures.set(index, fixture);
        await fixture.prepare();
        return Object.freeze({ turn: (offer) => this.turn(fixture, offer),
          read: (kind) => this.read(fixture, kind), resume: () => this.resume(fixture) });
      } finally { this.preparing = false; }
    });
  }
  turn(fixture, offer) {
    check(typeof offer === 'string' && /^[A-Za-z0-9_-]{1,80}$/.test(offer), 'offer_invalid');
    if (fixture.offers.has(offer)) return fixture.offers.get(offer).task;
    check(!this.stopped && fixture.ready && !fixture.active, 'agent_turn_unavailable');
    const turn = new Turn(fixture, offer); fixture.active = turn; fixture.offers.set(offer, turn);
    turn.task = this.track(() => turn.execute()).finally(() => { if (turn.settled) fixture.active = null; });
    return turn.task;
  }
  read(fixture, kind) {
    return this.track(async () => {
      check(!this.stopped && ['agents', 'history'].includes(kind), 'management_read_refused');
      await this.authority();
      const path = kind === 'agents' ? '/api/v1/agents?limit=20'
        : `/api/v1/agents/${fixture.agent}/transcript?limit=32`;
      const start = this.time.now(); const result = await fixture.get(path, start + 30000);
      return this.record({ scenario: 'management_read', status: 'PASS', elapsedMs: this.time.now() - start,
        evidenceHash: hash(JSON.stringify(result)) });
    });
  }
  resume(fixture) {
    return this.track(async () => {
      check(!fixture.active, 'resume_during_turn');
      const deadline = this.time.now() + SETUP_MS;
      await this.admit(fixture.spec, 'create', deadline);
      const action = `revive_${fixture.intents.size}`;
      await fixture.write(action, `/api/v1/sandboxes/${fixture.sandbox}/revive`, {}, deadline, [202]);
      await fixture.poll(`/api/v1/agents/${fixture.agent}`, deadline, ({ value }) => {
        fixture.checkAgent(value); return value.status === 'ready' && value.sandbox.status === 'active';
      });
      return { ok: true };
    });
  }
  async close() {
    this.closing = true;
    this.stop('scope_finished');
    const settleDeadline = this.time.now() + SETTLE_MS;
    // Every mutation is bounded; join late receipts before deciding what is owned.
    await Promise.allSettled([...this.pending]);
    await Promise.allSettled([...this.fixtures.values()].flatMap((f) =>
      [...f.offers.values()].filter((t) => !t.settled).map((t) => t.settle(settleDeadline))));
    await Promise.allSettled([...this.fixtures.values()].map((f) => f.primary?.close()));
    const cleanupDeadline = this.time.now() + CLEANUP_MS;
    const cleanup = [];
    for (const fixture of this.fixtures.values()) cleanup.push(await fixture.cleanup(cleanupDeadline));
    for (const connection of this.connections.values()) {
      await connection.close(); this.record({ scenario: 'http_accounting', status: 'PASS', ...connection.http });
    }
    const turns = [...this.fixtures.values()].flatMap((f) => [...f.offers.values()]);
    return { runHash: hash(this.config.runId), driverHash: this.config.driverHash,
      offered: this.offered, admitted: turns.filter((t) => t.state === 'admitted').length,
      completed: turns.filter((t) => t.passed).length, settledTurns: turns.filter((t) => t.settled).length,
      overdue: turns.filter((t) => t.overdue).length,
      unresolved: turns.filter((t) => t.state !== 'not_issued' && !t.settled).length,
      logicalCleanup: cleanup.every((r) => r.logical), physicalCleanup: false, cleanup: false };
  }
  facade() {
    const run = this;
    return Object.freeze({ create: (index) => run.create(index), stop: () => run.stop(), get stopped() { return run.stopped; } });
  }
  async scope(use) {
    let result;
    try { await use(this.facade()); }
    catch (error) {
      try { this.record({ scenario: 'run', ...safeFailure(error) }); }
      catch { /* An unavailable journal must not bypass the finalizer. */ }
    } finally { result = await this.close(); }
    return result;
  }
}
