import { EventEmitter } from 'node:events';
import { writeFile } from 'node:fs/promises';
import type { Page, Request, TestInfo } from '@playwright/test';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { StartupDiagnostics } from './startup-diagnostics';

vi.mock('node:fs/promises', () => {
  const writeFile = vi.fn().mockResolvedValue(undefined);
  return { writeFile, default: { writeFile } };
});

function harness() {
  const page = Object.assign(new EventEmitter(), {
    evaluate: vi.fn().mockResolvedValue({ readyState: 'complete', textboxes: [] }),
    screenshot: vi.fn().mockResolvedValue(Buffer.from('fixture')),
  });
  const info = {
    outputPath: vi.fn((name: string) => `/fixture/${name}`),
    attach: vi.fn().mockResolvedValue(undefined),
  };
  const diagnostics = new StartupDiagnostics(page as unknown as Page);
  const capture = () => diagnostics.capture(info as unknown as TestInfo, { queueReads: 0 });
  const payload = () => {
    return JSON.parse(String(vi.mocked(writeFile).mock.calls.at(-1)![1]));
  };
  return { page, info, diagnostics, capture, payload };
}

function request(path: string): Request {
  return {
    url: () => `http://127.0.0.1:1234${path}`,
    method: () => 'GET',
    resourceType: () => 'script',
    failure: () => ({ errorText: 'net::ERR_FAILED' }),
  } as Request;
}

afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

describe('startup failure evidence', () => {
  it('captures document milestones and the rendered composer state without field values', async () => {
    document.body.innerHTML = '<div id="root"><textarea disabled>private input</textarea></div>';
    vi.stubGlobal('performance', {
      now: () => 25,
      getEntriesByType: (type: string) =>
        type === 'navigation'
          ? [
              {
                startTime: 0,
                responseEnd: 5,
                domInteractive: 10,
                domContentLoadedEventEnd: 15,
                loadEventEnd: 20,
              },
            ]
          : [{ name: 'first-contentful-paint', startTime: 18 }],
    });
    const { page, diagnostics, capture, payload } = harness();
    page.evaluate.mockImplementation(async (callback: () => unknown) => callback());
    await capture();
    expect(payload().browser).toEqual(
      expect.objectContaining({
        rootChildren: 1,
        textboxes: [{ tag: 'TEXTAREA', disabled: true, visible: false }],
        navigation: [expect.objectContaining({ responseEnd: 5, loadEventEnd: 20 })],
        paint: [{ name: 'first-contentful-paint', startTime: 18 }],
      }),
    );
    expect(JSON.stringify(payload())).not.toContain('private input');
    diagnostics.dispose();
    document.body.innerHTML = '';
  });

  it('observes successful startup without evaluating, capturing or attaching artifacts', () => {
    const { page, info, diagnostics } = harness();
    diagnostics.mark('navigation-started');
    page.emit('domcontentloaded');
    page.emit('load');
    diagnostics.mark('textbox-enabled');
    diagnostics.dispose();
    expect(diagnostics.timeline.map((mark) => mark.name)).toEqual([
      'observer-started',
      'navigation-started',
      'domcontentloaded',
      'load',
      'textbox-enabled',
    ]);
    expect(page.evaluate).not.toHaveBeenCalled();
    expect(page.screenshot).not.toHaveBeenCalled();
    expect(info.attach).not.toHaveBeenCalled();
    expect(writeFile).not.toHaveBeenCalled();
    expect(page.eventNames()).toEqual([]);
  });

  it('retains failed and pending requests, console errors, milestones and the boundary', async () => {
    const { page, diagnostics, capture, payload } = harness();
    const asset = request('/assets/app.js?private=omitted#fragment');
    const failed = request('/assets/missing.js');
    const pending = request('/auth/session');
    page.emit('request', asset);
    page.emit('response', { request: () => asset, status: () => 200 });
    page.emit('requestfinished', asset);
    page.emit('request', failed);
    page.emit('requestfailed', failed);
    page.emit('request', pending);
    page.emit('console', { type: () => 'error', text: () => 'Synthetic startup error' });
    page.emit('pageerror', new Error('Synthetic render error'));
    page.emit('crash');
    await capture();
    const evidence = payload();
    expect(evidence.network).toEqual([
      expect.objectContaining({
        path: '/assets/app.js',
        status: 200,
        finishedMs: expect.any(Number),
      }),
      expect.objectContaining({ path: '/assets/missing.js', failure: 'net::ERR_FAILED' }),
      expect.objectContaining({ path: '/auth/session', startedMs: expect.any(Number) }),
    ]);
    expect(evidence.network[2]).not.toHaveProperty('finishedMs');
    expect(JSON.stringify(evidence)).not.toContain('private=');
    expect(evidence.console.map((entry: { type: string }) => entry.type)).toEqual([
      'error',
      'pageerror',
    ]);
    expect(evidence.timeline.map((entry: { name: string }) => entry.name)).toContain(
      'page-crashed',
    );
    expect(evidence.browser.textboxes).toEqual([]);
    expect(evidence.boundary.queueReads).toBe(0);
    expect(evidence.collectionErrors).toEqual([]);
    diagnostics.dispose();
  });

  it('bounds event storage and reports dropped observations', async () => {
    const { page, diagnostics, capture, payload } = harness();
    for (let index = 0; index < 510; index += 1) {
      page.emit('request', request(`/assets/${index}.js`));
      page.emit('console', { type: () => 'error', text: () => 'x'.repeat(3000) });
      diagnostics.mark('milestone');
    }
    await capture();
    const evidence = payload();
    expect(evidence.network).toHaveLength(500);
    expect(evidence.console).toHaveLength(100);
    expect(evidence.console[0].text).toHaveLength(2000);
    expect(evidence.timeline).toHaveLength(100);
    expect(evidence.dropped).toEqual({ network: 10, console: 410, timeline: 412 });
    diagnostics.dispose();
  });

  it('handles events outside the bounded request window and missing browser details', async () => {
    const { page, diagnostics, capture, payload } = harness();
    const unseen = request('/unobserved');
    page.emit('response', { request: () => unseen, status: () => 200 });
    page.emit('requestfinished', unseen);
    page.emit('requestfailed', unseen);
    const unknown = { ...request('/unknown'), failure: () => null } as Request;
    page.emit('request', unknown);
    page.emit('requestfailed', unknown);
    page.emit('pageerror', { message: 'error without stack' });
    document.body.innerHTML = '';
    vi.stubGlobal('performance', { now: () => 0, getEntriesByType: () => [] });
    page.evaluate.mockImplementation(async (callback: () => unknown) => callback());
    await capture();
    expect(payload().network).toHaveLength(1);
    expect(payload().network[0].failure).toBe('unknown');
    expect(payload().console[0].text).toBe('error without stack');
    expect(payload().browser.rootChildren).toBeNull();
    diagnostics.dispose();
  });

  it('retains Node evidence when the page has already closed', async () => {
    const { page, diagnostics, capture, payload } = harness();
    page.evaluate.mockRejectedValue(new Error('page closed'));
    page.screenshot.mockRejectedValue(new Error('page closed'));
    page.emit('close');
    await capture();
    expect(payload().collectionErrors).toEqual([
      'browser-state: Error: page closed',
      'screenshot: Error: page closed',
    ]);
    expect(payload().timeline.map((entry: { name: string }) => entry.name)).toContain(
      'page-closed',
    );
    diagnostics.dispose();
  });

  it('bounds renderer inspection so a hung page still emits Node evidence', async () => {
    vi.useFakeTimers();
    const { page, diagnostics, capture, payload } = harness();
    page.evaluate.mockReturnValue(new Promise(() => {}));
    const captured = capture();
    await vi.advanceTimersByTimeAsync(1000);
    await captured;
    expect(payload().collectionErrors).toEqual([
      'browser-state: Error: Browser state collection exceeded 1000ms',
    ]);
    diagnostics.dispose();
  });

  it('reports attachment failures without replacing the original assertion error', async () => {
    const { info, diagnostics, capture } = harness();
    info.attach.mockRejectedValue(new Error('artifact storage unavailable'));
    const report = vi.spyOn(console, 'error').mockImplementation(() => {});
    try {
      await expect(capture()).resolves.toBeUndefined();
      expect(report).toHaveBeenCalledWith(
        'Unable to attach startup diagnostics:',
        expect.any(Error),
      );
    } finally {
      diagnostics.dispose();
      report.mockRestore();
    }
  });
});
