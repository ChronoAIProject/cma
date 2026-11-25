import { randomUUID } from 'node:crypto';
import { writeFile } from 'node:fs/promises';
import { CheckError, requireCheck, poll, agentReady, hash, clock } from './core.mjs';
import { mark, observedApi } from './phase-hooks.mjs';

export async function paint(page) {
  mark(page, 'two_frames_wait_begin');
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  mark(page, 'two_frames_completed_observed');
}

export function deletionComplete(observation) {
  return observation.status === 404 || (observation.status === 200 && (
    Boolean(observation.body?.retired_at) || observation.body?.availability === 'retired'
    || ['retired', 'deleted', 'removed'].includes(observation.body?.status)));
}

export async function publicShell(page, origin) {
  const navigation = await page.goto(origin, { waitUntil: 'domcontentloaded', timeout: 30000 });
  requireCheck(navigation?.status() === 200, 'shell_http_failed', navigation?.status());
  await page.locator('#root > *').first().waitFor({ state: 'visible', timeout: 30000 });
  await page.locator('button:visible, a:visible, input:visible').first().waitFor({ timeout: 30000 });
  await paint(page);
  const artifacts = await page.locator('script[src]').evaluateAll((nodes) => nodes.map((node) => new URL(node.src).pathname).sort());
  return { http_status: 200, web_asset_set_sha256: hash(JSON.stringify(artifacts)) };
}

export async function session(context, origin, expectedSubject) {
  const response = await context.request.get(`${origin}/auth/session`, { maxRedirects: 0, timeout: 30000 });
  if (response.status() !== 200) {
    const problem = await response.json().catch(() => ({}));
    throw new CheckError('session_http_failed', response.status(), problem.code);
  }
  const value = await response.json();
  requireCheck(value.authenticated === true && value.visitor === false && typeof value.subject === 'string' && value.subject.length > 0
    && (expectedSubject === undefined || value.subject === expectedSubject), 'owner_session_mismatch');
  return value;
}

export function browserAllowed(url, method, origin, agentId, { messages = false, report = false } = {}, body) {
  if (url.origin !== origin) return false;
  if (method === 'POST' && url.pathname === '/auth/connections/check')
    return Boolean(agentId && body?.target?.type === 'agent' && body.target.id === agentId && body.active === undefined);
  if (url.pathname.startsWith('/auth/') && url.pathname !== '/auth/session') return false;
  if (['GET', 'HEAD', 'OPTIONS'].includes(method)) return true;
  if (report && method === 'POST' && ['/api/v1/admin/usage/metrics/reports', '/api/v1/admin/usage/metrics/options'].includes(url.pathname)) return true;
  if (!messages || method !== 'POST' || !agentId) return false;
  return url.pathname === `/api/v2/agents/${agentId}/responses`
    || new RegExp(`^/api/v2/agents/${agentId}/responses/[^/]+/controls$`).test(url.pathname);
}

export async function guardPage(page, origin, agentId, permissions) {
  let denied = 0;
  await page.route('**/*', async (route) => {
    const request = route.request();
    let body;
    try { body = request.postDataJSON(); } catch { /* A non-JSON request cannot pass the connection check. */ }
    if (!browserAllowed(new URL(request.url()), request.method(), origin, agentId, permissions, body)) {
      denied += 1;
      return route.abort('blockedbyclient');
    }
    return route.continue();
  });
  // No WebSocket control is part of the readiness benchmark.
  await page.routeWebSocket('**/*', (socket) => socket.close());
  return () => denied;
}

export async function openAgent(page, origin, id) {
  mark(page, 'navigation_invoked');
  const response = await page.goto(`${origin}/agents/${id}`, { waitUntil: 'domcontentloaded', timeout: 30000 });
  mark(page, 'navigation_returned');
  requireCheck(response?.status() === 200, 'agent_navigation_failed', response?.status());
  await page.locator('.agent-surface__conversation').waitFor({ timeout: 30000 });
  mark(page, 'conversation_visible');
  await page.waitForFunction(() => !document.querySelector('.agent-surface__history-notice'), null, { timeout: 30000 });
  mark(page, 'history_notice_hidden');
  const input = page.getByRole('textbox', { name: 'Message', exact: true });
  await input.fill('e2e-slo-unsent-readiness-check', { timeout: 30000 });
  mark(page, 'draft_fill_complete');
  await poll(() => page.getByRole('button', { name: 'Send message', exact: true }).isEnabled(), Boolean, { timeout: 30000, interval: 20 });
  mark(page, 'send_enabled');
  await input.fill('');
  mark(page, 'draft_clear_complete');
  await paint(page);
  mark(page, 'interactive');
  return { conversation_rendered: true, history_loading_finished: true, composer_usable: true };
}

export async function sendMessage(page, origin, id, message) {
  mark(page, 'fill_invoked');
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill(message);
  mark(page, 'fill_complete');
  const receipt = page.waitForResponse((response) => {
    const request = response.request();
    return request.method() === 'POST' && new URL(response.url()).pathname === `/api/v2/agents/${id}/responses` && request.postDataJSON()?.input === message && response.status() === 200;
  }, { timeout: 30000 });
  // Suppress unhandled rejections if a click fails before awaiting the response.
  receipt.catch(() => undefined);
  mark(page, 'click_invoked');
  await page.getByRole('button', { name: 'Send message', exact: true }).click({ timeout: 30000 });
  mark(page, 'click_returned');
  const accepted = await receipt;
  mark(page, 'receipt_headers');
  // These are durable command identifiers, not credentials. Never record their raw values.
  const responseId = await accepted.headerValue('x-cma-response-id');
  const commandId = await accepted.headerValue('x-cma-command-id');
  requireCheck(responseId && commandId, 'missing_durable_message_receipt');
  mark(page, 'receipt_parsed');
  await page.locator('.agent-surface__conversation').getByText(message, { exact: true }).waitFor({ timeout: 30000 });
  mark(page, 'message_visible');
  await poll(() => page.getByRole('textbox', { name: 'Message', exact: true }).inputValue(), (value) => value === '', { timeout: 30000, interval: 20 });
  mark(page, 'composer_clear');
  await paint(page);
  mark(page, 'acceptance');
  return { responseId, evidence: { accepted: true, message_rendered: true, response_hash: hash(responseId) } };
}

export async function stopMessage(page, origin, agentId, responseId) {
  const started = clock();
  const path = `/api/v2/agents/${agentId}/responses/${responseId}/controls`;
  const receipt = page.waitForResponse((response) => {
    const request = response.request();
    return request.method() === 'POST' && new URL(response.url()).pathname === path && request.postDataJSON()?.command?.type === 'cancel';
  }, { timeout: 30000 });
  receipt.catch(() => undefined);
  mark(page, 'click_invoked');
  try { await page.getByRole('button', { name: 'Stop response', exact: true }).click({ timeout: 30000 }); }
  catch { throw new CheckError('stop_control_not_clickable'); }
  mark(page, 'click_returned');
  let accepted;
  try { accepted = await receipt; } catch { throw new CheckError('stop_receipt_timeout'); }
  requireCheck(accepted.status() === 202, 'stop_request_refused', accepted.status());
  const acknowledgement_ms = clock() - started;
  mark(page, 'receipt_headers');
  const body = await accepted.json();
  mark(page, 'receipt_parsed');
  requireCheck(body.target?.response_id === responseId && body.control?.receipt?.agent_id === agentId && body.control?.receipt?.command_id, 'stop_receipt_mismatch');
  await poll(async () => {
    const path = `/api/v2/commands/${body.control.receipt.command_id}`;
    const response = await observedApi(page, 'GET', path, () => page.request.get(`${origin}${path}`, { timeout: 10000, maxRedirects: 0 }));
    requireCheck(response.status() === 200, 'stop_status_failed', response.status());
    const command = await response.json();
    requireCheck(command.receipt?.command_id === body.control.receipt.command_id &&
      command.receipt?.agent_id === agentId, 'stop_command_mismatch');
    requireCheck(!['failed', 'cancelled'].includes(command.status), 'stop_delivery_failed');
    return command.status === 'completed';
  }, Boolean, { timeout: 30000, interval: 100 });
  const authoritative_confirmation_ms = clock() - started;
  mark(page, 'command_completed');
  try { await page.getByRole('button', { name: 'Stop response', exact: true }).waitFor({ state: 'hidden', timeout: 30000 }); }
  catch { throw new CheckError('stop_ui_completion_timeout'); }
  const stop_hidden_ms = clock() - started;
  mark(page, 'stop_hidden');
  await paint(page);
  mark(page, 'acceptance');
  return { inner_timer_started_monotonic_ms: started, acknowledgement_received: true, acknowledgement_ms, authoritative_confirmation_ms, stop_hidden_ms, delivery_completed: true, stopped_ui_rendered: true };
}

export async function adminReport(page, origin) {
  let reportState;
  const observe = async (response) => {
    const path = new URL(response.url()).pathname;
    if (/^\/api\/v1\/admin\/usage\/metrics\/reports(?:\/[^/]+)?$/.test(path) && [200, 202].includes(response.status())) {
      try { reportState = (await response.json()).status; } catch { /* No response bodies enter artifacts. */ }
    }
  };
  page.on('response', observe);
  const rows = page.waitForResponse((response) => response.request().method() === 'GET'
    && /^\/api\/v1\/admin\/usage\/metrics\/reports\/[^/]+\/sandboxes$/.test(new URL(response.url()).pathname)
    && response.status() === 200, { timeout: 60000 });
  rows.catch(() => undefined);
  await page.goto(`${origin}/operations?tab=usage&range=7d`, { waitUntil: 'domcontentloaded', timeout: 30000 });
  let rowResponse;
  try { rowResponse = await rows; }
  catch { throw new CheckError(reportState === 'pending' ? 'usage_report_pending_timeout' : 'usage_report_rows_timeout'); }
  finally { page.off('response', observe); }
  const data = await rowResponse.json();
  requireCheck(Array.isArray(data.items), 'usage_page_invalid');
  const dashboard = page.locator('[aria-label="Operations resource dashboard"]');
  await dashboard.waitFor({ timeout: 60000 });
  await dashboard.getByRole('region', { name: 'Allocated capacity now' }).waitFor({ timeout: 60000 });
  const inventory = dashboard.getByRole('region', { name: 'Usage inventory', exact: true });
  await inventory.waitFor({ timeout: 60000 });
  if (data.items.length) {
    await poll(() => inventory.locator('tbody tr').count(), (count) => count === data.items.length, { timeout: 60000, interval: 20 });
  } else await inventory.getByText('No Sandboxes match this selection.', { exact: true }).waitFor({ timeout: 60000 });
  await poll(() => dashboard.getByText(/Preparing usage report|Loading usage report|Loading usage page|Showing the previous selection/).count(), (count) => count === 0, { timeout: 60000, interval: 20 });
  await paint(page);
  return { first_page_rendered: true, summary_rendered: true, row_count: data.items.length };
}

export class OwnedResources {
  constructor(context, origin, journalPath) {
    this.context = context; this.origin = origin; this.journalPath = journalPath;
    this.record = { name: `e2e-slo-${Date.now()}-${randomUUID().slice(0, 8)}`, creationKey: randomUUID(), state: 'prepared' };
  }
  async save() { await writeFile(this.journalPath, JSON.stringify(this.record, null, 2), { mode: 0o600 }); }
  async read(path, timeout = 30000) {
    const response = await this.context.request.get(`${this.origin}${path}`, { timeout: Math.min(timeout, 30000), maxRedirects: 0 });
    const value = await response.json().catch(() => ({}));
    return { status: response.status(), body: response.status() === 200 ? value : null, problemCode: response.status() !== 200 ? value.code : undefined };
  }
  async create(desktop) {
    this.record.desktop = desktop; this.record.state = 'submitting'; await this.save();
    let response;
    try {
      response = await this.context.request.post(`${this.origin}/api/v1/agents`, {
        data: { name: this.record.name, machine_included: desktop },
        headers: { Origin: this.origin, 'Idempotency-Key': this.record.creationKey }, timeout: 30000, maxRedirects: 0,
      });
    } catch { throw new CheckError('creation_outcome_unknown'); }
    if (response.status() !== 202) {
      // Only a definitive 4xx refusal proves no resources were admitted.
      if ([400, 401, 403, 404, 422, 429].includes(response.status())) this.record.state = 'refused';
      await this.save();
      const problem = await response.json().catch(() => ({}));
      throw new CheckError('creation_not_accepted', response.status(), problem.code);
    }
    this.record.operationId = (await response.json()).id;
    this.record.admissionStatus = response.status();
    requireCheck(typeof this.record.operationId === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(this.record.operationId), 'creation_receipt_invalid');
    this.record.state = 'admitted'; await this.save();
    await this.resolveOperation();
    const agent = await poll(async (remaining) => {
      const detail = await this.read(`/api/v1/agents/${this.record.agentId}`, remaining);
      requireCheck(detail.status === 200, 'agent_detail_failed', detail.status);
      this.bindAgentDetail(detail.body);
      await this.save();
      return detail.body;
    }, agentReady);
    requireCheck(agent.id === this.record.agentId && this.record.sandboxId, 'created_identity_mismatch');
    requireCheck(agent.desktop === desktop, 'creation_variant_mismatch');
    this.record.state = 'ready'; await this.save();
    return { operation_succeeded: true, agent_ready: true, sandbox_active: true, desktop_verified: agent.desktop };
  }
  bindAgentDetail(agent) {
    requireCheck(agent.id === this.record.agentId && /^sbx_[a-f0-9]{32}$/.test(agent.sandbox?.id ?? '') &&
      (!this.record.sandboxId || this.record.sandboxId === agent.sandbox.id), 'created_identity_mismatch');
    this.record.sandboxId = agent.sandbox.id;
  }
  async resolveOperation(timeout = 120000) {
    return poll(async (remaining) => {
      const observation = await this.read(`/api/v1/agent-lifecycle/${this.record.operationId}`, remaining);
      requireCheck(observation.status === 200, 'operation_read_failed', observation.status, observation.problemCode);
      const operation = observation.body;
      requireCheck(operation.id === this.record.operationId, 'creation_operation_mismatch');
      if (/^[a-z_]{1,64}$/.test(operation.status)) this.record.lastOperationStatus = operation.status;
      if (/^[a-z_]{1,64}$/.test(operation.phase)) this.record.lastOperationPhase = operation.phase;
      if (operation.agent_id) {
        requireCheck(/^agt_[a-f0-9]{32}$/.test(operation.agent_id) &&
          (!this.record.agentId || this.record.agentId === operation.agent_id), 'operation_agent_identity_invalid');
        this.record.agentId = operation.agent_id; await this.save();
      }
      requireCheck(!['failed', 'waiting_for_connection'].includes(operation.status), 'creation_terminal_failure');
      return operation;
    }, (operation) => operation.status === 'succeeded' && Boolean(operation.agent_id), { timeout, interval: 250 });
  }
  async cleanup() {
    if (['prepared', 'refused', 'deleted_verified'].includes(this.record.state)) return;
    // Never create the next cohort while this operation or its cleanup is unresolved.
    if (!this.record.operationId) throw new CheckError('cleanup_creation_unknown');
    const operation = await poll(async (remaining) => {
      const current = await this.read(`/api/v1/agent-lifecycle/${this.record.operationId}`, remaining);
      requireCheck(current.status === 200, 'cleanup_operation_read_failed', current.status);
      requireCheck(current.body.id === this.record.operationId, 'cleanup_operation_mismatch');
      if (current.body.agent_id) {
        requireCheck(/^agt_[a-f0-9]{32}$/.test(current.body.agent_id) &&
          (!this.record.agentId || this.record.agentId === current.body.agent_id), 'cleanup_agent_identity_invalid');
        this.record.agentId = current.body.agent_id;
        await this.save();
      }
      return current.body;
    }, (value) => ['succeeded', 'failed'].includes(value.status), { timeout: 300000, interval: 1000 });
    requireCheck(operation.agent_id && this.record.agentId, 'cleanup_operation_unresolved');
    const detail = await this.read(`/api/v1/agents/${this.record.agentId}`);
    if (detail.status === 200) this.bindAgentDetail(detail.body);
    else requireCheck(detail.status === 404, 'cleanup_agent_read_failed', detail.status);
    requireCheck(/^sbx_[a-f0-9]{32}$/.test(this.record.sandboxId ?? ''), 'cleanup_sandbox_unknown');
    await this.save();
    // Match CMA's production E2E cleanup contract: stop active Agents first,
    // and accept verified terminal tombstones as well as physical absence.
    if (detail.status === 200 && !['stopped', 'failed', 'deleted'].includes(detail.body.status)) {
      const stopped = await this.context.request.post(`${this.origin}/api/v1/agents/${this.record.agentId}/stop`, {
        data: {}, headers: { Origin: this.origin, 'Idempotency-Key': `${this.record.creationKey}-stop` },
        timeout: 30000, maxRedirects: 0,
      });
      requireCheck(stopped.ok(), 'cleanup_stop_failed', stopped.status());
      await poll(async (remaining) => {
        const current = await this.read(`/api/v1/agents/${this.record.agentId}`, remaining);
        requireCheck([200, 404].includes(current.status), 'cleanup_stop_read_failed', current.status);
        return current.status === 404 || ['stopped', 'failed', 'deleted'].includes(current.body.status);
      }, Boolean, { timeout: 90000, interval: 1000 });
    }
    for (const [kind, id] of [['agents', this.record.agentId], ['sandboxes', this.record.sandboxId]]) {
      const response = await this.context.request.delete(`${this.origin}/api/v1/${kind}/${id}`, {
        headers: { Origin: this.origin, 'Idempotency-Key': `${this.record.creationKey}-delete-${kind}` }, timeout: 30000, maxRedirects: 0,
      });
      requireCheck([200, 202, 204, 404].includes(response.status()), 'cleanup_delete_failed', response.status());
      await poll(async (remaining) => {
        const read = await this.read(`/api/v1/${kind}/${id}`, remaining);
        requireCheck([200, 404].includes(read.status), 'cleanup_verification_failed', read.status);
        const complete = deletionComplete(read);
        if (complete) {
          this.record[`${kind}_deletion`] = { http_status: read.status, terminal_status: read.body?.status ?? 'absent' };
          await this.save();
        }
        return complete;
      }, Boolean, { timeout: 300000, interval: 1000 });
    }
    this.record.state = 'deleted_verified';
    delete this.record.creationKey;
    await this.save();
  }
}
