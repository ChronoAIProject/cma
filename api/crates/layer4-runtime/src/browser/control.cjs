const { createHash, randomUUID } = require('node:crypto');

function refused(code = 'browser_control_conflict') {
  return Object.assign(new Error(code), { code });
}

function canonical(value) {
  if (value === null || typeof value !== 'object') return JSON.stringify(value);
  return '{' + Object.keys(value).sort().filter(key => value[key] !== undefined)
    .map(key => JSON.stringify(key) + ':' + canonical(value[key])).join(',') + '}';
}

class BrowserControl {
  constructor() {
    this.revision = 0;
    this.lease = null;
    this.receipts = new Map();
    this.audit = [];
  }

  status(request = {}, receipt) {
    this.expire();
    return { state: this.lease ? 'user' : 'agent', revision: this.revision,
      client_id: this.lease?.client_id ?? null,
      lease_token: receipt && receipt.lease_id === this.lease?.token && request.actor === this.lease?.actor
        && request.client_id === this.lease?.client_id ? this.lease.token : null,
      operation_id: receipt ? request.operation_id : null, operation_revision: receipt?.revision ?? null,
      expires_at: this.lease?.expires_at ?? null, lease_duration_ms: 15000,
      audit: this.audit, audit_truncated_before: this.audit[0]?.revision > 1 ? this.audit[0].revision : null };
  }

  requireAgent() {
    this.expire();
    if (this.lease) throw refused();
  }

  extend() {
    this.lease.deadline = performance.now() + 15000;
    this.lease.expires_at = new Date(Date.now() + 15000).toISOString();
  }

  record(kind, request) {
    this.revision += 1;
    this.audit.push({ revision: this.revision, kind, actor: request.actor, client_id: request.client_id,
      operation_id: request.operation_id ?? null, at: new Date().toISOString() });
    if (this.audit.length > 128) this.audit.shift();
  }

  expire() {
    if (this.lease && performance.now() >= this.lease.deadline) {
      this.record('expired', this.lease);
      this.lease = null;
    }
  }

  async dispatch(request, input) {
    this.expire();
    const previous = this.receipts.get(request.operation_id);
    if (request.command === 'status') {
      if (request.operation_id && (!previous || previous.actor !== request.actor || previous.client_id !== request.client_id))
        throw refused('browser_operation_unknown');
      return this.status(request, previous);
    }
    if (typeof request.operation_id !== 'string' || request.operation_id.length > 64
      || typeof request.actor !== 'string' || !request.actor || request.actor.length > 128
      || typeof request.client_id !== 'string' || !request.client_id || request.client_id.length > 64)
      throw refused('browser_control_invalid');
    const { id, ...payload } = request;
    const fingerprint = createHash('sha256').update(canonical(payload)).digest('hex');
    if (previous) {
      if (previous.fingerprint !== fingerprint) throw refused();
      return this.status(request, previous);
    }
    if (request.expected_revision !== this.revision) throw refused();
    let kind;
    if (request.command === 'acquire') {
      this.requireAgent();
      this.lease = { actor: request.actor, client_id: request.client_id, token: randomUUID() };
      this.extend();
      kind = 'acquired';
    } else {
      if (!this.lease || request.lease_token !== this.lease.token || request.actor !== this.lease.actor
        || request.client_id !== this.lease.client_id) throw refused();
      if (request.command === 'return') { this.lease = null; kind = 'returned'; }
      else if (request.command === 'input') { await input(request.input); kind = 'input'; }
      else if (request.command === 'renew') { this.extend(); kind = 'renewed'; }
      else throw refused('browser_control_invalid');
    }
    this.record(kind, request);
    const receipt = { fingerprint, revision: this.revision, actor: request.actor, client_id: request.client_id, lease_id: this.lease?.token ?? null };
    this.receipts.set(request.operation_id, receipt);
    if (this.receipts.size > 128) this.receipts.delete(this.receipts.keys().next().value);
    return this.status(request, receipt);
  }
}

module.exports = { BrowserControl, refused };
