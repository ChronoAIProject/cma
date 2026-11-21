import { writeFile } from 'node:fs/promises';
import type { Page, Request, TestInfo } from '@playwright/test';

interface NetworkEntry {
  method: string;
  path: string;
  resource: string;
  startedMs: number;
  responseMs?: number;
  status?: number;
  finishedMs?: number;
  failure?: string;
}

/** Only for synthetic loopback fixtures: never records headers, bodies or query values. */
export class StartupDiagnostics {
  readonly timeline: { name: string; atMs: number }[] = [];
  readonly network: NetworkEntry[] = [];
  readonly console: { type: string; text: string; atMs: number }[] = [];
  readonly dropped = { network: 0, console: 0, timeline: 0 };
  private readonly pending = new Map<Request, NetworkEntry>();
  private readonly started = performance.now();

  constructor(private readonly page: Page) {
    page.on('request', this.request);
    page.on('response', this.response);
    page.on('requestfinished', this.finished);
    page.on('requestfailed', this.failed);
    page.on('console', this.message);
    page.on('pageerror', this.pageError);
    page.on('crash', this.crash);
    page.on('domcontentloaded', this.domContentLoaded);
    page.on('load', this.load);
    page.on('close', this.closed);
    this.mark('observer-started');
  }

  mark(name: string) {
    if (this.timeline.length < 100) this.timeline.push({ name, atMs: this.now() });
    else this.dropped.timeline += 1;
  }

  private now() {
    return Math.round((performance.now() - this.started) * 10) / 10;
  }

  private request = (request: Request) => {
    if (this.network.length >= 500) {
      this.dropped.network += 1;
      return;
    }
    const entry = {
      method: request.method(),
      path: new URL(request.url()).pathname,
      resource: request.resourceType(),
      startedMs: this.now(),
    };
    this.network.push(entry);
    this.pending.set(request, entry);
  };

  private response = (response: import('@playwright/test').Response) => {
    const entry = this.pending.get(response.request());
    if (entry) Object.assign(entry, { status: response.status(), responseMs: this.now() });
  };

  private finished = (request: Request) => {
    const entry = this.pending.get(request);
    if (entry) entry.finishedMs = this.now();
    this.pending.delete(request);
  };

  private failed = (request: Request) => {
    const entry = this.pending.get(request);
    if (entry) {
      entry.finishedMs = this.now();
      entry.failure = request.failure()?.errorText ?? 'unknown';
    }
    this.pending.delete(request);
  };

  private record(type: string, text: string) {
    if (this.console.length < 100)
      this.console.push({ type, text: text.slice(0, 2000), atMs: this.now() });
    else this.dropped.console += 1;
  }

  private message = (message: import('@playwright/test').ConsoleMessage) =>
    this.record(message.type(), message.text());
  private pageError = (error: Error) => this.record('pageerror', error.stack ?? error.message);
  private crash = () => this.mark('page-crashed');
  private domContentLoaded = () => this.mark('domcontentloaded');
  private load = () => this.mark('load');
  private closed = () => this.mark('page-closed');

  /** Call in catch, before the fixture closes its page; never replaces the original failure. */
  async capture(info: TestInfo, boundary: unknown) {
    this.mark('failure-captured');
    const collectionErrors: string[] = [];
    let browser: unknown;
    try {
      browser = await this.browserState();
    } catch (error) {
      collectionErrors.push(`browser-state: ${String(error)}`);
    }
    try {
      const path = info.outputPath('startup-failure.png');
      await this.page.screenshot({ path, timeout: 2000 });
      await info.attach('startup-screenshot', { path, contentType: 'image/png' });
    } catch (error) {
      collectionErrors.push(`screenshot: ${String(error)}`);
    }
    try {
      // The list reporter does not persist inline attachment bodies on its own.
      const path = info.outputPath('startup-diagnostics.json');
      await writeFile(
        path,
        JSON.stringify(
          {
            clock: 'milliseconds since observer setup; browser timings since navigation',
            timeline: this.timeline,
            network: this.network,
            console: this.console,
            dropped: this.dropped,
            browser,
            boundary,
            collectionErrors,
          },
          null,
          2,
        ),
      );
      await info.attach('startup-diagnostics', {
        path,
        contentType: 'application/json',
      });
    } catch (error) {
      // Preserve the assertion failure even when the artifact directory is unavailable.
      console.error('Unable to attach startup diagnostics:', error);
    }
  }

  private async browserState() {
    let timer: ReturnType<typeof setTimeout> | undefined;
    // A wedged renderer must not prevent the already-observed Node events being saved.
    const deadline = new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => reject(new Error('Browser state collection exceeded 1000ms')), 1000);
    });
    try {
      return await Promise.race([
        deadline,
        this.page.evaluate(() => ({
          readyState: document.readyState,
          visibilityState: document.visibilityState,
          rootChildren: document.getElementById('root')?.childElementCount ?? null,
          textboxes: [...document.querySelectorAll('textarea, input, [role="textbox"]')].map(
            (element) => ({
              tag: element.tagName,
              disabled: element.matches(':disabled, [aria-disabled="true"]'),
              visible: element.getClientRects().length > 0,
            }),
          ),
          navigation: performance.getEntriesByType('navigation').map((entry) => {
            const timing = entry as PerformanceNavigationTiming;
            return {
              startTime: timing.startTime,
              responseEnd: timing.responseEnd,
              domInteractive: timing.domInteractive,
              domContentLoadedEventEnd: timing.domContentLoadedEventEnd,
              loadEventEnd: timing.loadEventEnd,
            };
          }),
          paint: performance.getEntriesByType('paint').map(({ name, startTime }) => ({
            name,
            startTime,
          })),
        })),
      ]);
    } finally {
      clearTimeout(timer);
    }
  }

  dispose() {
    this.page.off('request', this.request);
    this.page.off('response', this.response);
    this.page.off('requestfinished', this.finished);
    this.page.off('requestfailed', this.failed);
    this.page.off('console', this.message);
    this.page.off('pageerror', this.pageError);
    this.page.off('crash', this.crash);
    this.page.off('domcontentloaded', this.domContentLoaded);
    this.page.off('load', this.load);
    this.page.off('close', this.closed);
    this.pending.clear();
  }
}
