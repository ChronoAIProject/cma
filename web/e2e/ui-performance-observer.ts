import { writeFile } from 'node:fs/promises';
import type { Page, Request, TestInfo } from '@playwright/test';

interface Read {
  method: string;
  path: string;
  phase: string;
  startMs: number;
  endMs?: number;
  status?: number;
  failed?: boolean;
}

/** Aggregate boundary timings only: no headers, bodies, query values or credentials. */
export class PerformanceObserver {
  readonly requests: Read[] = [];
  readonly marks: Record<string, number> = {};
  readonly errors: string[] = [];
  private readonly pending = new Map<Request, Read>();
  private readonly started = performance.now();
  private phase = 'load';

  constructor(private readonly page: Page) {
    page.on('pageerror', (error) => this.errors.push(error.message));
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (!path.startsWith('/api/') && !path.startsWith('/auth/')) return;
      const read = { method: request.method(), path, phase: this.phase, startMs: this.now() };
      this.pending.set(request, read);
      this.requests.push(read);
    });
    page.on('response', (response) => {
      const read = this.pending.get(response.request());
      if (read) read.status = response.status();
    });
    page.on('requestfinished', (request) => {
      const read = this.pending.get(request);
      if (read) read.endMs = this.now();
    });
    page.on('requestfailed', (request) => {
      const read = this.pending.get(request);
      if (read) Object.assign(read, { endMs: this.now(), failed: true });
    });
  }

  now() {
    return Math.round(performance.now() - this.started);
  }

  mark(name: string) {
    this.marks[name] = this.now();
  }

  begin(phase: string) {
    this.phase = phase;
    this.mark(`${phase}Start`);
  }

  /** Register last so every local public boundary has the same synthetic RTT. */
  async delay() {
    await this.page.route(/\/(api|auth)\//, async (route) => {
      await new Promise((resolve) => setTimeout(resolve, 100));
      await route.fallback();
    });
  }

  async save(info: TestInfo) {
    const payload = {
      environment: 'compiled loopback build; recorded public fixtures; 100ms boundary delay',
      browser: info.project.name,
      marks: this.marks,
      requests: this.requests,
      scriptErrors: this.errors,
      paint: await this.page.evaluate(() =>
        performance.getEntriesByType('paint').map((entry) => ({
          name: entry.name,
          startMs: entry.startTime,
        })),
      ),
    };
    const path = info.outputPath('waterfall.json');
    await writeFile(path, JSON.stringify(payload, null, 2));
    await info.attach('waterfall', { path, contentType: 'application/json' });
  }
}
