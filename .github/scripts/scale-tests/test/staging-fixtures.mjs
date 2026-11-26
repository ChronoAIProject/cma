import { hash } from '../lib/checks.mjs';
import { StagingRun } from '../staging/run.mjs';
import { fixtureName } from '../staging/authority.mjs';
import { safeRow } from '../staging/evidence.mjs';
import { transcriptPage } from './staging-transcript-fixture.mjs';

export const incarnation = hash('synthetic-incarnation');
export const profiles = {
  sandbox: { profile_id: 'sbp_synthetic', revision: 1, content: { workspace_profiles: [] }, digest: hash('sandbox') },
  agent: { profile_id: 'agp_synthetic', revision: 1, content: { model: 'synthetic-model' }, digest: hash('agent') },
};
export function config() {
  return { schema: 'cma-staging-driver-v1', environment: 'staging', profile: 'staging', transport: 'browser-cookie',
    origin: 'https://staging.invalid', nyxidOrigin: 'https://identity.invalid', runId: 'synthetic-run',
    expiresAt: new Date(Date.now() + 3600000).toISOString(),
    ...Object.fromEntries(['candidateHash', 'serviceMapHash', 'policyHash', 'approvalHash', 'driverHash', 'reviewHash']
      .map((key) => [key, hash(key)])), reviewVerdict: 'GO',
    policy: { toolsDisabled: true, networkDisabled: true, writesDisabled: true, enforcementHash: hash('enforced') },
    maxTurns: 30, maxTurnLiability: 1, modelCap: 100, infrastructureCap: 100,
    fixtures: [{ index: 1, subject: 'synthetic-owner', browserSample: true, createPerMinute: 600, readPerMinute: 600,
      sandboxProfile: { id: profiles.sandbox.profile_id, revision: 1, sha256: hash(JSON.stringify(profiles.sandbox)) },
      agentProfile: { id: profiles.agent.profile_id, revision: 1, sha256: hash(JSON.stringify(profiles.agent)) },
      storageStatePath: '/private/synthetic-state', storageStateHash: hash('synthetic-state') }],
  };
}
export function authority(c = config()) {
  return { ...c, schema: 'cma-staging-driver-authority-v1', observedAt: new Date().toISOString(),
    subjectHashes: c.fixtures.map((f) => hash(f.subject)), stop: false, healthReady: true, providerReady: true,
    browserReady: true, capacityHomes: 200, infrastructureReserved: 1, spentAndReserved: 0 };
}
export function frames(prompt, mode = 'complete', offset = 0) {
  const answer = prompt.slice('Only reply '.length).split('. Do not')[0];
  const binding = { command_id: `cmd_synthetic_${offset}`, response_id: `rsp_synthetic_${offset}`, turn_sequence: 2 + offset };
  const durable = (relative, kind, extra = {}) => { const sequence = relative + offset; return ({ event: 'cma.agent.event', id: `ag1.${incarnation}.${sequence}`,
    data: { agent_id: 'agt_synthetic', incarnation, sequence, cursor: `ag1.${incarnation}.${sequence}`,
      appended_at: '2026-01-01T00:00:00Z', kind, response_binding: null, ...extra } }); };
  const rows = [durable(1, { type: 'message', role: 'user', content: prompt }, { message_command_id: binding.command_id }),
    durable(2, { type: 'turn_started', turn_sequence: 2 + offset }, { response_binding: binding })];
  if (mode === 'echo_only') return rows;
  rows.push({ event: 'cma.agent.live', data: { agent_id: 'agt_synthetic', incarnation,
    turn_sequence: 2 + offset, after_sequence: 2 + offset, message_id: 'synthetic-message', kind: 'text_delta', delta: answer } });
  rows.push(durable(3, { type: 'message', role: 'assistant', content: mode === 'wrong_nonce' ? 'OK different' : answer }));
  rows.push(durable(4, mode === 'failed' ? { type: 'turn_failed', turn_sequence: 2 + offset, failure: { class: 'test', detail: 'test' } }
    : { type: 'turn_completed', turn_sequence: 2 + offset }));
  if (mode === 'wrong_agent') rows[0].data.agent_id = 'agt_foreign';
  if (mode === 'gap') rows.splice(2, 0, { event: 'cma.agent.live_gap', data: { agent_id: 'agt_synthetic',
    incarnation, turn_sequence: 2 + offset, frames: 1, reason: 'lagged' } });
  if (mode === 'missing_binding') delete rows[0].data.message_command_id;
  return rows;
}
export const encodeFrames = (rows) => rows.map((r) => `${r.id ? `id: ${r.id}\n` : ''}event: ${r.event}\ndata: ${JSON.stringify(r.data)}\n\n`).join('');
export class FakeHttp {
  constructor(options = {}) {
    this.options = options; this.calls = []; this.prompts = []; this.http = { started: 0, completed: 0, failed: 0, open: 0, streamOpened: 0, streamClosed: 0 };
    this.sandbox = { id: 'sbx_synthetic', name: fixtureName(1), owner: 'synthetic-owner',
      profile: { id: profiles.sandbox.profile_id, revision: 1 }, status: 'active' };
    this.workspace = { id: 'wsp_synthetic', sandbox_id: this.sandbox.id, name: fixtureName(1), status: 'ready' };
    this.agent = { id: 'agt_synthetic', title: fixtureName(1), profile: { id: profiles.agent.profile_id, revision: 1 },
      sandbox: { id: this.sandbox.id, status: 'active' }, workspace: { id: this.workspace.id },
      worktree: { id: 'wtr_synthetic' }, status: 'ready' };
  }
  async session() { if (this.options.wrongSession) throw new Error('session'); }
  async observeAgent() { if (this.options.browserFails) throw new Error('observer'); }
  async visible() { if (this.options.visibleFails) throw new Error('visible'); }
  async request(path, options = {}) {
    this.calls.push({ path, ...options }); this.http.started++; this.http.completed++;
    const result = (value, status = 200) => ({ status, value: structuredClone(value) });
    const method = options.method ?? 'GET';
    if (path.includes('-profiles/')) return result(path.includes('sandbox-profiles') ? profiles.sandbox : profiles.agent);
    if (method === 'POST' && path === '/api/v1/sandboxes') {
      if (this.options.failCreate === 'sandbox') throw new Error('lost receipt');
      return result({ ...this.sandbox, ...(this.options.wrongOwner ? { owner: 'foreign-owner' } : {}) }, 201);
    }
    if (method === 'POST' && path.endsWith('/workspaces/inline')) {
      if (this.options.failCreate === 'workspace') throw new Error('lost receipt');
      return result(this.workspace, 201);
    }
    if (method === 'POST' && path.endsWith('/agents')) {
      if (this.options.failCreate === 'agent') throw new Error('lost receipt');
      return result(this.agent, 201);
    }
    if (method === 'POST' && path.endsWith('/input')) {
      this.prompt = JSON.parse(options.body).text;
      const bytes = new TextEncoder().encode(encodeFrames(frames(this.prompt, this.options.mode, this.prompts.length * 4)));
      this.prompts.push(this.prompt); this.controller.enqueue(bytes);
      if (this.options.lostAck) throw new Error('ack lost');
      return result({ queued: true }, 202);
    }
    if (path.endsWith('/events/head')) return result({ object: 'cma.agent.event_head', schema_version: 1,
      agent_id: this.agent.id, incarnation, through_cursor: `ag1.${incarnation}.0`, active_turn: null });
    if (path.includes('/transcript')) {
      if (!this.prompt || this.options.mode === 'echo_only') return result({ agent_id: this.agent.id, turns: [], through_seq: 0 });
      return result(transcriptPage(path, this.prompts.map((prompt, i) => ({
        turn_sequence: i * 4 + 2, started_at: '2026-01-01T00:00:00Z', owner_message: prompt,
        messages: [{ sequence: i * 4 + 1, role: 'user', content: prompt },
          { sequence: i * 4 + 3, role: 'assistant', content: this.options.badDurable ? 'bad answer'
            : prompt.slice('Only reply '.length).split('. Do not')[0] }],
        outcome: { type: 'completed', at: '2026-01-01T00:00:01Z' },
      })), this.prompts.length * 4));
    }
    if (path.startsWith('/api/v2/commands/')) {
      const offset = Number(path.split('_').at(-1));
      return result({ receipt: { command_id: `cmd_synthetic_${offset}`, agent_id: this.agent.id,
        response_id: `rsp_synthetic_${offset}` }, turn_sequence: offset + 2, status: 'completed' });
    }
    if (path.endsWith('/stop')) {
      if (this.options.stopFails) throw new Error('stop failed');
      this.agent.status = 'stopped'; return result({}, 202);
    }
    if (method === 'DELETE' && path.includes('/agents/')) { this.agent.status = 'deleted'; return result({}, 202); }
    if (method === 'DELETE' && path.includes('/sandboxes/')) {
      this.sandbox.status = 'retired'; this.agent.status = 'deleted'; return result(this.sandbox, 202);
    }
    if (path.includes('/sandboxes/')) return result(this.sandbox);
    if (path.includes('/workspaces/')) return result(this.workspace);
    if (path.includes('/worktrees/')) return result({ id: 'wtr_synthetic', workspace_id: this.workspace.id,
      sandbox_id: this.sandbox.id, agent_id: this.agent.id });
    if (path.includes('/agents/')) return result(this.agent);
    throw new Error('unexpected synthetic route');
  }
  async stream(_path, signal) {
    this.http.streamOpened++;
    return new Response(new ReadableStream({ start: (controller) => {
      this.controller = controller;
      controller.enqueue(new TextEncoder().encode(encodeFrames([{ event: 'cma.agent.live_gap', data: {
        agent_id: this.agent.id, incarnation, turn_sequence: null, frames: null, reason: 'reconnected' } }])));
      signal.addEventListener('abort', () => {
        try { controller.close(); } catch { /* Parser may already have cancelled the body. */ }
        this.http.streamClosed++;
      }, { once: true });
    } }), { headers: { 'content-type': 'text/event-stream' } });
  }
  async close() { this.closed = true; }
}
export function fakeRun(wire, options = {}) {
  const c = config(); const rows = []; const http = new FakeHttp(options);
  const time = { value: 0, now() { return this.value; }, async sleep(ms) { this.value += ms; await new Promise(setImmediate); } };
  const run = new StagingRun(c, { wire, time, readAuthority: async () => authority(c), connect: async () => http,
    evidence: { record: (row) => { const safe = safeRow(row); rows.push(safe); return safe; } } });
  return { run, http, rows, time };
}
