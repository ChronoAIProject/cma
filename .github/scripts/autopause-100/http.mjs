import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {API, hash, poll, wait, assertOracle} from '../multi-am-features/cma/core.mjs';
import {createAdapter} from '../multi-am-features/owner/adapter.mjs';
import {awaitCreatedIdle, isNativeSuspended} from './lifecycle.mjs';

export class PauseAdapter {
  constructor(config, observe) {
    for (const origin of Object.values(config.ams)) {
      const url = new URL(origin);
      assert(url.protocol === 'http:' && url.hostname === '127.0.0.1', 'local API required');
    }
    const control = new URL(config.control_origin);
    assert(control.protocol === 'http:' && control.hostname === '127.0.0.1', 'local control required');
    this.config = config;
    this.api = new API(config, observe);
    this.admin = new API({...config, bearer: config.admin_bearer}, observe);
    this.fixture = createAdapter(config);
    this.receipts = [];
    this.observe = observe;
  }

  async read(path, admin = false) {
    return (await (admin ? this.admin : this.api).request('a', 'GET', path)).body;
  }
  async settings() { return this.read('/api/v1/admin/settings', true); }
  async sandbox(record) { return (await this.read(`/api/v1/admin/sandboxes/${record.sandboxId}`, true)).sandbox; }
  async home(record) { return this.fixture.runtimeFor({agentId: record.agentId}); }

  async prepare() {
    this.originalSettings = await this.settings();
    assert.equal(this.originalSettings.settings.inactivity.pause_after_seconds, 900, 'default idle policy');
    const created = await this.api.request('a', 'POST', '/api/v1/agents', {
      body: {name: 'autopause-100-' + randomUUID(), machine_included: false, desktop: false},
      key: randomUUID(), allowed: [202]});
    const operationId = created.body.id;
    const op = await poll(() => this.read(`/api/v1/agent-lifecycle/${operationId}`), x => {
      assert(!['failed', 'waiting_for_connection'].includes(x.status), 'Agent creation failed');
      return x.status === 'succeeded' && x.agent_id;
    }, {interval: 2000, timeout: 120000, code: 'create_timeout'});
    const agentId = op.agent_id;
    const detail = await awaitCreatedIdle(() => this.read(`/api/v1/agents/${agentId}`));
    const record = {agentId, sandboxId: detail.sandbox.id, operationId, file: 'autopause-sentinel.txt'};
    this.record = record;
    record.home = await this.home(record);
    record.content = 'original home sentinel ' + randomUUID();
    record.sha256 = hash(record.content);
    const uploaded = await this.api.request('a', 'POST', `/api/v1/agents/${agentId}/files`, {
      body: {path: record.file, content_base64: Buffer.from(record.content).toString('base64')}});
    assert.equal(uploaded.body.sha256, record.sha256);
    // First prove the public default, then accelerate only the supported setting.
    const current = await this.settings();
    const updated = await this.admin.request('a', 'PATCH', '/api/v1/admin/settings', {body: {
      expected_revision: current.revision, inactivity: {auto_pause_enabled: true, pause_after_seconds: 60}}});
    assert.equal(updated.body.settings.inactivity.pause_after_seconds, 60);
    assert(updated.body.settings.inactivity.grace_started_at, 'server grace missing');
    this.observe({kind: 'policy', default_seconds: 900, accelerated_seconds: 60,
      grace_started_at: updated.body.settings.inactivity.grace_started_at});
    return record;
  }

  async alwaysOnException(record) {
    const publicPath = `/api/v1/sandboxes/${record.sandboxId}/always-on`;
    const before = await this.sandbox(record);
    const denied = await this.api.request('a', 'POST', publicPath, {
      body: {always_on: true, expected_revision: before.revision}, key: randomUUID(), allowed: [403]});
    assert(JSON.stringify(denied.body).includes('always_on_approval_required'), 'owner denial not typed');
    const path = `/api/v1/admin/sandboxes/${record.sandboxId}/always-on`;
    await this.admin.request('a', 'POST', path, {body: {always_on: true, expected_revision: before.revision}});
    const start = performance.now();
    let samples = 0;
    while (performance.now() - start < 90000) {
      const current = await this.sandbox(record);
      assert.equal(current.status, 'active', 'Always-on exception paused');
      assert.equal(current.profile.always_on, true);
      samples += 1;
      await wait(5000);
    }
    const current = await this.sandbox(record);
    await this.admin.request('a', 'POST', path, {body: {always_on: false, expected_revision: current.revision}});
    this.observe({kind: 'always_on', verdict: 'PASS', hold_ms: performance.now() - start, samples});
  }

  async awaitAutomaticPause(record) {
    const started = performance.now();
    let samples = 0;
    const snapshot = await poll(async () => {
      const value = await this.sandbox(record); samples += 1;
      assert(!['failed', 'deleted'].includes(value.status), 'sandbox failed while idle');
      return value;
    }, value => value.status === 'paused', {interval: 2000, timeout: 120000, code: 'automatic_pause_timeout'});
    const home = await poll(() => this.home(record), isNativeSuspended,
      {interval: 2000, timeout: 30000, code: 'paused_runtime_not_released'});
    return {elapsed_ms: performance.now() - started, samples, revision: snapshot.revision,
      last_activity_at: snapshot.last_activity_at, observed_at: new Date().toISOString(),
      runtime_pods: home.pods.length, native_operating_mode: home.operating_mode,
      native_generation: home.generation};
  }

  async wakeWithMessage(record, index) {
    const caseId = 'autopause-' + index + '-' + randomUUID();
    const marker = 'Reply exactly with ' + caseId;
    await this.fixture.prepareTurn({caseId, marker});
    const stream = await this.api.send('a', record.agentId, marker, caseId);
    const path = `/api/v2/agents/${record.agentId}/responses/${stream.responseId}/controls`;
    const controls = await poll(() => this.read(path), value => value.target_active === true,
      {interval: 2000, timeout: 120000, code: 'message_wake_timeout'});
    assert.equal(controls.response.receipt.agent_id, record.agentId);
    assert.equal(controls.response.receipt.response_id, stream.responseId);
    await poll(() => this.fixture.oracle({caseId}), value => value.request_count === 1 && value.active_streams === 1,
      {interval: 2000, timeout: 120000, code: 'provider_not_reached'});
    return {caseId, marker, stream, path};
  }

  async holdActiveTurn(record, turn) {
    const start = performance.now();
    let samples = 0;
    while (performance.now() - start < 90000) {
      const current = await this.sandbox(record);
      assert.equal(current.status, 'active', 'active turn interrupted by pause');
      const controls = await this.read(turn.path);
      assert.equal(controls.target_active, true, 'held turn ceased to be active');
      const oracle = await this.fixture.oracle({caseId: turn.caseId});
      assert.equal(oracle.request_count, 1);
      assert.equal(oracle.completed_effect_count, 0);
      assert.equal(oracle.active_streams, 1);
      samples += 1;
      await wait(5000);
    }
    return {hold_ms: performance.now() - start, samples, interrupted: false};
  }

  async completeTurn(record, turn) {
    await this.fixture.releaseTurn({caseId: turn.caseId});
    const final = await poll(() => this.read(`/api/v2/responses/${turn.stream.responseId}`),
      x => ['completed', 'failed', 'incomplete', 'cancelled'].includes(x.status),
      {interval: 2000, timeout: 120000, code: 'reply_timeout'});
    assert.equal(final.status, 'completed', 'message did not complete');
    const output = final.output.filter(item => item.type === 'message')
      .flatMap(item => item.content.filter(part => part.type === 'output_text').map(part => part.text));
    assert.equal(output.filter(text => text.includes(turn.marker)).length, 1, 'reply missing or duplicated');
    await turn.stream.done;
    assert.equal(turn.stream.error, null, 'message SSE failed');
    assertOracle(await this.fixture.oracle({caseId: turn.caseId}), turn.caseId, 1);
    const controls = await this.read(turn.path);
    const response = {id: final.id, receipt: controls.response.receipt, status: final.status,
      output_sha256: hash(JSON.stringify(final.output))};
    assert.equal(final.conversation.id, record.agentId);
    this.receipts.push(response);
    return response;
  }

  async verifyReceipt(record, response) {
    const state = await this.read(`/api/v2/agents/${record.agentId}/responses/${response.id}/controls`);
    assert.deepEqual(state.response.receipt, response.receipt, 'original receipt changed');
    assert.equal(state.response.status, 'completed');
    const final = await this.read(`/api/v2/responses/${response.id}`);
    assert.equal(final.conversation.id, record.agentId, 'original conversation changed');
    assert.equal(hash(JSON.stringify(final.output)), response.output_sha256, 'original reply changed');
  }

  async verifyRetention(record) {
    const file = await this.read(`/api/v1/agents/${record.agentId}/files/content?path=${record.file}`);
    assert.equal(file.sha256, record.sha256, 'original file changed');
    assert.equal(Buffer.from(file.content_base64, 'base64').toString(), record.content);
    await this.verifyReceipt(record, this.receipts[0]);
    const operation = await this.read(`/api/v1/agent-lifecycle/${record.operationId}`);
    assert.equal(operation.agent_id, record.agentId);
    assert.equal(operation.status, 'succeeded');
  }

  async verifyAllReceipts(record) {
    for (const response of this.receipts) await this.verifyReceipt(record, response);
  }

  async cleanup() {
    const errors = [];
    if (this.originalSettings) {
      try {
        const current = await this.settings();
        const {grace_started_at, ...inactivity} = this.originalSettings.settings.inactivity;
        await this.admin.request('a', 'PATCH', '/api/v1/admin/settings', {
          body: {expected_revision: current.revision, inactivity}});
      } catch (error) { errors.push({step: 'restore_policy', error: error.message}); }
    }
    await this.api.close();
    await this.admin.close();
    return errors;
  }
}
