import { check, hash, safeFailure } from '../lib/checks.mjs';
import { SETUP_MS, identity, fixtureName } from './authority.mjs';
import { PrimaryObservation } from './observation.mjs';

const reference = (p) => ({ id: p.id, revision: p.revision });
export class Fixture {
  constructor(run, spec, http) {
    Object.assign(this, { run, spec, http });
    this.name = fixtureName(spec.index); this.intents = new Map(); this.offers = new Map();
    this.ready = false; this.active = null;
  }
  row(value) {
    return this.run.record({ slot: this.spec.index, subjectHash: hash(this.spec.subject),
      nameHash: hash(this.name), ...value });
  }
  async get(path, deadline, expected = [200]) {
    return this.http.request(path, { timeoutMs: this.run.remaining(deadline), expected });
  }
  async poll(path, deadline, accept, expected = [200]) {
    while (this.run.remaining(deadline)) {
      const result = await this.get(path, deadline, expected);
      if (accept(result)) return result;
      await this.run.time.sleep(Math.min(500, this.run.remaining(deadline)));
    }
  }
  async write(action, path, body, deadline, expected = [201], method = 'POST') {
    check(!this.intents.has(action), 'write_already_issued');
    const key = `tl-scale-${hash(`${this.run.config.runId}:${this.spec.index}:${action}`)}`;
    const intent = { key, body: body === undefined ? undefined : JSON.stringify(body), state: 'issued' };
    this.intents.set(action, intent);
    this.row({ scenario: action, status: 'issued', keyHash: hash(key), requestHash: hash(intent.body ?? '') });
    let result;
    try {
      result = await this.http.request(path, { method, body: intent.body, key,
        expected, timeoutMs: this.run.remaining(deadline) });
      this.acquire(action, result.value);
    } catch (error) {
      intent.state = 'uncertain'; this.row({ scenario: action, status: 'uncertain', ...safeFailure(error) });
      throw error;
    }
    intent.state = 'acknowledged'; intent.receiptHash = hash(JSON.stringify(result));
    this.row({ scenario: action, status: intent.state, receiptHash: intent.receiptHash });
    return result.value;
  }
  acquire(action, value) {
    // Validate and retain known ownership synchronously, before logging can fail.
    if (action === 'create_sandbox') {
      this.checkSandbox(value); this.sandbox = value.id;
    } else if (action === 'create_workspace') {
      check(identity(value?.id) && value.sandbox_id === this.sandbox && value.name === this.name,
        'workspace_identity_mismatch');
      this.workspace = value.id;
    } else if (action === 'create_agent') {
      this.checkAgent(value); this.agent = value.id;
    } else return;
    this.intents.get(action).verified = true;
  }
  checkSandbox(value) {
    check(identity(value?.id) && value.name === this.name && value.owner === this.spec.subject
      && value.profile?.id === this.spec.sandboxProfile.id
      && value.profile?.revision === this.spec.sandboxProfile.revision, 'sandbox_identity_mismatch');
    if (this.sandbox) check(value.id === this.sandbox, 'sandbox_identity_changed');
  }
  checkAgent(value) {
    check(identity(value?.id) && (value.owner_title ?? value.title) === this.name
      && value.sandbox?.id === this.sandbox && value.workspace?.id === this.workspace
      && identity(value.worktree?.id)
      && value.profile?.id === this.spec.agentProfile.id
      && value.profile?.revision === this.spec.agentProfile.revision, 'agent_lineage_mismatch');
    if (this.agent) check(value.id === this.agent, 'agent_identity_changed');
  }
  async prepare() {
    const deadline = this.run.time.now() + SETUP_MS;
    await this.http.session();
    for (const [kind, p] of [['sandbox', this.spec.sandboxProfile], ['agent', this.spec.agentProfile]]) {
      const { value } = await this.get(`/api/v1/${kind}-profiles/${p.id}/revisions/${p.revision}`, deadline);
      check(hash(JSON.stringify(value)) === p.sha256, 'fixture_profile_changed');
      if (kind === 'sandbox') check(Array.isArray(value.content?.workspace_profiles)
        && value.content.workspace_profiles.length === 0, 'automatic_children_refused');
    }
    await this.run.admit(this.spec, 'sandbox', deadline);
    await this.write('create_sandbox', '/api/v1/sandboxes',
      { sandbox_profile: reference(this.spec.sandboxProfile), name: this.name }, deadline);
    this.row({ scenario: 'sandbox_owned', status: 'PASS', resourceHash: hash(this.sandbox) });
    await this.poll(`/api/v1/sandboxes/${this.sandbox}`, deadline, ({ value }) => {
      this.checkSandbox(value); check(!['failed', 'retired'].includes(value.status), 'sandbox_setup_failed');
      return value.status === 'active';
    });
    await this.run.admit(this.spec, 'create', deadline);
    await this.write('create_workspace', `/api/v1/sandboxes/${this.sandbox}/workspaces/inline`,
      { source: { type: 'empty' }, name: this.name }, deadline);
    await this.poll(`/api/v1/workspaces/${this.workspace}`, deadline, ({ value }) => {
      check(value.id === this.workspace && value.sandbox_id === this.sandbox, 'workspace_identity_changed');
      check(!['failed', 'removed'].includes(value.status), 'workspace_setup_failed'); return value.status === 'ready';
    });
    await this.run.admit(this.spec, 'create', deadline);
    const agent = await this.write('create_agent', `/api/v1/workspaces/${this.workspace}/agents`,
      { agent_profile: reference(this.spec.agentProfile), title: this.name }, deadline);
    this.row({ scenario: 'agent_owned', status: 'PASS', resourceHash: hash(this.agent) });
    const { value: worktree } = await this.get(`/api/v1/worktrees/${agent.worktree.id}`, deadline);
    check(worktree.id === agent.worktree.id && worktree.workspace_id === this.workspace
      && worktree.sandbox_id === this.sandbox && worktree.agent_id === this.agent, 'worktree_lineage_mismatch');
    await this.poll(`/api/v1/agents/${this.agent}`, deadline, ({ value }) => {
      this.checkAgent(value); check(!['failed', 'deleted', 'stopped'].includes(value.status), 'agent_setup_failed');
      return value.status === 'ready' && value.sandbox.status === 'active';
    });
    this.primary = new PrimaryObservation(this);
    await this.primary.start(deadline);
    if (this.spec.browserSample) await this.http.observeAgent(this.agent, this.run.remaining(deadline));
    this.run.remaining(deadline); this.ready = true;
    this.row({ scenario: 'fixture_ready', status: 'PASS' });
  }
  async cleanupAgent(deadline) {
    if (!this.agent) return;
    const path = `/api/v1/agents/${this.agent}`;
    const agent = await this.get(path, deadline, [200, 404]);
    if (agent.status === 404) return;
    this.checkAgent(agent.value);
    if (!['stopped', 'failed', 'deleted'].includes(agent.value.status)) {
      await this.run.quota(this.spec, 'create', deadline);
      await this.write('stop_agent', `${path}/stop`, {}, deadline, [202]);
      await this.poll(path, deadline, ({ status, value }) => {
        if (status === 404) return true; this.checkAgent(value);
        return ['stopped', 'failed', 'deleted'].includes(value.status);
      }, [200, 404]);
    }
    await this.run.quota(this.spec, 'create', deadline);
    await this.write('delete_agent', path, undefined, deadline, [202, 404], 'DELETE');
  }
  async cleanup(deadline) {
    let sandboxTerminal = !this.sandbox; let agentTerminal = !this.agent;
    try {
      await this.run.authority(true); await this.http.session();
      if (this.sandbox) {
        const sandbox = await this.get(`/api/v1/sandboxes/${this.sandbox}`, deadline, [200, 404]);
        if (sandbox.status === 200) this.checkSandbox(sandbox.value);
      }
    } catch (error) {
      this.row({ scenario: 'cleanup_authority', ...safeFailure(error), cleanup: false });
      return { logical: false, cleanup: false };
    }
    try { await this.cleanupAgent(deadline); }
    catch (error) { this.row({ scenario: 'cleanup_agent', ...safeFailure(error) }); }
    try {
      if (this.sandbox) {
        await this.run.authority(true); await this.run.quota(this.spec, 'create', deadline);
        await this.write('retire_sandbox', `/api/v1/sandboxes/${this.sandbox}`, undefined, deadline, [202, 404], 'DELETE');
        const result = await this.poll(`/api/v1/sandboxes/${this.sandbox}`, deadline, ({ status, value }) => {
          if (status === 404) return true; this.checkSandbox(value); return value.status === 'retired';
        }, [200, 404]);
        sandboxTerminal = true;
        this.row({ scenario: 'sandbox_terminal', status: 'terminal', evidenceHash: hash(JSON.stringify(result)) });
      }
    } catch (error) { this.row({ scenario: 'cleanup_sandbox', ...safeFailure(error) }); }
    try {
      if (this.agent) {
        const result = await this.poll(`/api/v1/agents/${this.agent}`, deadline, ({ status, value }) => {
          if (status === 404) return true; this.checkAgent(value); return value.status === 'deleted';
        }, [200, 404]);
        agentTerminal = true;
        this.row({ scenario: 'agent_terminal', status: 'terminal', evidenceHash: hash(JSON.stringify(result)) });
      }
    } catch (error) { this.row({ scenario: 'cleanup_agent_terminal', ...safeFailure(error) }); }
    const logical = sandboxTerminal && agentTerminal
      && [...this.intents.entries()].filter(([action]) => action.startsWith('create_')).every(([, i]) => i.verified);
    this.row({ scenario: 'cleanup', status: logical ? 'BLOCKED' : 'FAIL', logicalCleanup: logical,
      physicalCleanup: false, cleanup: false, code: logical ? 'external_resource_release_required' : 'custody_unresolved' });
    return { logical, cleanup: false };
  }
}
