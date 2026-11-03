const { spawn } = require('node:child_process');

class DevTools {
  constructor(profile) {
    this.pending = new Map();
    this.sequence = 0;
    this.closed = false;
    this.child = spawn(process.env.CMA_BROWSER_BINARY || '/usr/bin/chromium', [
      '--headless', '--remote-debugging-pipe', '--no-first-run', '--no-default-browser-check',
      '--disable-background-networking', '--disable-sync', '--disable-extensions',
      '--disable-dev-shm-usage', '--window-size=1280,720', '--user-data-dir=' + profile,
    ], { stdio: ['ignore', 'ignore', 'pipe', 'pipe', 'pipe'] });
    this.child.stderr.resume();
    this.child.once('error', () => this.stop());
    this.child.once('exit', () => this.stop());
    let buffered = '';
    this.child.stdio[4].setEncoding('utf8');
    this.child.stdio[4].on('data', chunk => {
      buffered += chunk;
      if (buffered.length > 2 * 1024 * 1024) { this.stop(); return; }
      while (buffered.includes('\0')) {
        const boundary = buffered.indexOf('\0');
        const text = buffered.slice(0, boundary);
        buffered = buffered.slice(boundary + 1);
        let packet;
        try { packet = JSON.parse(text); } catch { this.stop(); return; }
        const waiting = this.pending.get(packet.id);
        if (!waiting) continue;
        this.pending.delete(packet.id);
        clearTimeout(waiting.deadline);
        if (packet.error) waiting.reject(new Error('Browser command refused'));
        else waiting.resolve(packet.result);
      }
    });
  }

  call(method, params = {}, sessionId) {
    if (this.closed || this.pending.size >= 8) return Promise.reject(new Error('Browser disconnected'));
    const identifier = ++this.sequence;
    return new Promise((resolve, reject) => {
      const deadline = setTimeout(() => {
        this.pending.delete(identifier);
        reject(new Error('Browser deadline exceeded'));
        this.stop();
      }, 10000);
      this.pending.set(identifier, { resolve, reject, deadline });
      this.child.stdio[3].write(JSON.stringify({ id: identifier, method, params, sessionId }) + '\0',
        error => { if (error) this.stop(); });
    });
  }

  stop() {
    if (this.closed) return this.stopped ?? Promise.resolve();
    this.closed = true;
    for (const waiting of this.pending.values()) {
      clearTimeout(waiting.deadline);
      waiting.reject(new Error('Browser disconnected'));
    }
    this.pending.clear();
    this.stopped = this.child.exitCode !== null || this.child.signalCode !== null || !this.child.pid
      ? Promise.resolve() : new Promise(resolve => {
        const deadline = setTimeout(() => this.child.kill('SIGKILL'), 1000);
        this.child.once('exit', () => { clearTimeout(deadline); resolve(); });
        this.child.kill('SIGTERM');
      });
    return this.stopped;
  }
}

module.exports = { DevTools };
