const { randomUUID } = require('node:crypto');

function refused(code = 'browser_control_conflict') {
  return Object.assign(new Error(code), { code });
}

class BrowserControl {
  constructor() {
    this.revision = 0;
    this.lease = null;
  }

  status() {
    return { state: this.lease ? 'user' : 'agent', revision: this.revision,
      client_id: this.lease?.client_id ?? null, lease_token: this.lease?.token ?? null };
  }

  requireAgent() {
    if (this.lease) throw refused();
  }

  async dispatch(request, input) {
    if (request.expected_revision !== this.revision) throw refused();
    if (request.command === 'acquire') {
      this.requireAgent();
      this.lease = { actor: request.actor, client_id: request.client_id, token: randomUUID() };
    } else {
      if (!this.lease || request.lease_token !== this.lease.token || request.actor !== this.lease.actor
        || request.client_id !== this.lease.client_id) throw refused();
      if (request.command === 'return') this.lease = null;
      else if (request.command === 'input') await input(request.input);
      else throw refused('browser_control_invalid');
    }
    this.revision += 1;
    return this.status();
  }
}

module.exports = { BrowserControl, refused };
