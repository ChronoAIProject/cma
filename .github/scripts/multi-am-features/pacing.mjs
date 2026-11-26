import {createHash} from 'node:crypto';

// The production defaults allow 10 Create POSTs and 600 other requests/minute.
// Pace before owner observation; never retry a rejected or uncertain mutation.
export class RequestPacer {
  constructor({now = () => performance.now(), sleep = ms => new Promise(r => setTimeout(r, ms)),
    createMs = 6200, defaultMs = 120} = {}) {
    Object.assign(this, {now, sleep, createMs, defaultMs});
    this.next = new Map();
  }
  async wait(credential, method) {
    const tier = method === 'POST' ? 'create' : 'default';
    const key = createHash('sha256').update(credential).digest('hex') + ':' + tier;
    if (!this.next.has(key) && this.next.size >= 64) throw new Error('fixture_pacing_identity_bound');
    const current = this.now(), at = Math.max(current, this.next.get(key) ?? current);
    this.next.set(key, at + (tier === 'create' ? this.createMs : this.defaultMs));
    if (at > current) await this.sleep(at - current);
  }
}

export const requestPacer = new RequestPacer();
