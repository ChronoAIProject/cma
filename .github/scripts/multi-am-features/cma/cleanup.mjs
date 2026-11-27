import {writeFile} from 'node:fs/promises';
import {join} from 'node:path';
import {randomUUID} from 'node:crypto';
import {check, hash, id, lineage, other, poll, safeError} from './core.mjs';

export async function cleanupFixture({record, config, api, killedAm, result}) {
  if (!record.agentId || record.deleted) return;
  check(record.operationId, 'cleanup_create_lineage_incomplete');
  const am = killedAm ? other(killedAm) : other(record.am);
  const receiptPath = join(config.output_dir, 'cleanup-' + hash(record.agentId) + '-' + randomUUID() + '.private.json');
  const journal = {am, agent_id:record.agentId, sandbox_id:record.sandboxId, started_at:new Date().toISOString(), mutations:[], last_reads:{}};
  const persist = () => writeFile(receiptPath, JSON.stringify(journal,null,2)+'\n', {mode:0o600});
  async function cleanupRequest(target, method, path, options) {
    try {
      const response = record.sharedSandbox
        ? await api.rawRequest(target, method, path, options)
        : await api.request(target, method, path, options);
      const entry = {at:new Date().toISOString(), method, path, ...response};
      if (method === 'GET') journal.last_reads[path] = entry;
      else {journal.mutations.push(entry); await persist();}
      return response;
    } catch(error) {
      journal.request_error = {at:new Date().toISOString(), method, path, error:safeError(error), response:error.response ?? null};
      await persist(); throw error;
    }
  }
  async function cleanupDetail() {
    const response = await cleanupRequest(am, 'GET', `/api/v1/agents/${record.agentId}`);
    // A timed-out operation can assign the Agent before the journey reads its Sandbox.
    const sandboxId = record.sandboxId ?? response.body?.sandbox?.id;
    check(id(sandboxId) && lineage({...record, sandboxId}, response.body), 'agent_lineage_mismatch');
    record.sandboxId = sandboxId;
    journal.sandbox_id = sandboxId;
    return response.body;
  }
  try {
    const op = await cleanupRequest(am, 'GET', `/api/v1/agent-lifecycle/${record.operationId}`);
    check(op.body.id === record.operationId && op.body.agent_id === record.agentId, 'cleanup_lifecycle_mismatch');
    const current = await cleanupDetail();
    if (!['stopped', 'deleted'].includes(current.status)) {
      await cleanupRequest(am, 'POST', `/api/v1/agents/${record.agentId}/stop`, {body: {}, key: record.key + '-stop', allowed: [200, 202, 204]});
      await poll(() => cleanupDetail(), value => value.status === 'stopped', {timeout: 180000, code: 'agent_stop_timeout'});
    }
    const deleteKey = record.key + '-delete';
    const deleteReceipt = await cleanupRequest(am, 'DELETE', `/api/v1/agents/${record.agentId}`,
      {key: deleteKey, allowed: [200, 202, 204]});
    const deleted = await poll(() => cleanupRequest(am, 'GET', `/api/v1/agents/${record.agentId}`, {allowed: [200, 404]}), value => value.status === 404 || value.body?.status === 'deleted', {timeout: 180000});
    if (record.sharedSandbox) {
      check(deleted.status === 200 && deleted.body?.id === record.agentId, 'shared_agent_deletion_unproven');
      record.deleted = true;
      return;
    }
    const sandbox = await cleanupRequest(am, 'GET', `/api/v1/sandboxes/${record.sandboxId}`);
    check(sandbox.body?.id === record.sandboxId && sandbox.body.owner === config.owner_id, 'sandbox_owner_mismatch');
    const suffix = ' sandbox ' + record.operationId;
    check(sandbox.body.name === record.name.slice(0, Math.max(0, 64 - suffix.length)) + suffix, 'sandbox_operation_name_mismatch');
    let cursor; const seen = new Set();
    do {
      const query = new URLSearchParams({limit: '200', ...(cursor ? {cursor} : {})});
      const page = await cleanupRequest(am, 'GET', '/api/v1/agents?' + query);
      check(Array.isArray(page.body?.items), 'agent_inventory_missing');
      check(!page.body.items.some(x => x.sandbox_id === record.sandboxId && x.status !== 'deleted'), 'sandbox_has_live_agent');
      cursor = page.body.next; if (cursor) {check(typeof cursor === 'string' && !seen.has(cursor) && seen.size < 20, 'inventory_incomplete'); seen.add(cursor);}
    } while (cursor);
    await cleanupRequest(am, 'DELETE', `/api/v1/sandboxes/${record.sandboxId}`, {key: record.key + '-retire', allowed: [200, 202, 204]});
    await poll(() => cleanupRequest(am, 'GET', `/api/v1/sandboxes/${record.sandboxId}`, {allowed: [200, 404]}), value => value.status === 404 || value.body?.status === 'retired', {timeout: 300000, code: 'sandbox_retire_timeout'});
    record.deleted = true; result.cleanup.push({agent_sha256: hash(record.agentId), sandbox_sha256: hash(record.sandboxId), public_stop_delete_retire: 'PASS', am});
    return {deleteKey, deleteReceipt};
  } catch(error) {
    journal.failure = safeError(error); throw error;
  } finally {
    journal.finished_at = new Date().toISOString(); await persist();
    (result.cleanup_evidence ??= []).push({am, agent_sha256:hash(record.agentId), native_response_path:receiptPath,
      delete_status:journal.mutations.find(x => x.method === 'DELETE' && x.path === `/api/v1/agents/${record.agentId}`)?.status ?? null,
      last_agent_status:journal.last_reads[`/api/v1/agents/${record.agentId}`]?.body?.status ?? null,
      failure:journal.failure ?? null});
  }
}
