import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  acquirePlayback,
  animationAllowed,
  boundedViewport,
  finishPlayback,
  hasInteractionFocus,
  reportTopographicFault,
} from './topographicPolicy';
import { topographicEnabled } from './topographicFeature';

let now = 100;
beforeEach(() => vi.spyOn(performance, 'now').mockImplementation(() => now));
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  now = 100;
});

function submit(lease: NonNullable<ReturnType<typeof acquirePlayback>>) {
  const frame = lease.takeFrame();
  return frame?.submit(() => {}) ? frame.elapsed : null;
}

describe('document lifetime and the real submission grant', () => {
  it('accounts for slow preparation and slow draw completion at the submission boundary', () => {
    const doc = document.implementation.createHTMLDocument();
    const lease = acquirePlayback(doc, 'landing')!;
    const first = lease.takeFrame()!;
    now += 200;
    expect(
      first.submit(() => {
        now += 50;
      }),
    ).toBe(true);
    now += 33;
    expect(lease.takeFrame()).toBeNull();
    now++;
    const prepared = lease.takeFrame()!;
    now = 4100;
    const draw = vi.fn();
    expect(prepared.submit(draw)).toBe(false);
    expect(draw).not.toHaveBeenCalled();
  });
  it('retains the original deadline and global frame spacing across remounts', () => {
    const doc = document.implementation.createHTMLDocument();
    const first = acquirePlayback(doc, 'first-visit')!;
    expect(submit(first)).toBe(0);
    expect(submit(first)).toBeNull();
    now += 33;
    expect(submit(first)).toBeNull();
    first.release();
    const second = acquirePlayback(doc, 'first-visit')!;
    expect(submit(first)).toBeNull();
    expect(second.remaining()).toBe(3967);
    expect(submit(second)).toBeNull();
    now++;
    expect(submit(second)).toBe(0.034);
    now = 4099;
    expect(submit(second)).toBe(3.999);
    now++;
    expect(submit(second)).toBeNull();
    expect(second.remaining()).toBe(0);
    second.release();
    expect(acquirePlayback(doc, 'first-visit')).toBeNull();
  });
  it('counts background time and never grants catch-up frames', () => {
    const doc = document.implementation.createHTMLDocument();
    const lease = acquirePlayback(doc, 'landing')!;
    expect(submit(lease)).toBe(0);
    now += 3500;
    expect(submit(lease)).toBe(3.5);
    expect(submit(lease)).toBeNull();
    now += 501;
    expect(submit(lease)).toBeNull();
  });
  it('allows only one active owner and keeps spacing across placement handoffs', () => {
    const doc = document.implementation.createHTMLDocument();
    const home = acquirePlayback(doc, 'first-visit')!;
    expect(acquirePlayback(doc, 'landing')).toBeNull();
    submit(home);
    home.release();
    const landing = acquirePlayback(doc, 'landing')!;
    home.release();
    expect(submit(landing)).toBeNull();
    now += 34;
    expect(submit(landing)).toBe(0.034);
    landing.finish();
    expect(submit(landing)).toBeNull();
    landing.release();
    expect(acquirePlayback(doc, 'landing')).toBeNull();
  });
  it('makes interaction or policy stops permanent even before a first draw', () => {
    const doc = document.implementation.createHTMLDocument();
    finishPlayback(doc, 'landing');
    expect(acquirePlayback(doc, 'landing')).toBeNull();
    const lease = acquirePlayback(doc, 'first-visit')!;
    finishPlayback(doc, 'first-visit');
    expect(submit(lease)).toBeNull();
  });
  it('records one fixed category per document', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const doc = document.implementation.createHTMLDocument();
    reportTopographicFault(doc, 'context_lost');
    reportTopographicFault(doc, 'render_failed');
    expect(warn).toHaveBeenCalledExactlyOnceWith('Topographic decoration:', 'context_lost');
  });
});

it.each([undefined, '', 'false', 'TRUE', '1'])('is inert for flag %s', (value) => {
  vi.stubEnv('VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED', value);
  expect(topographicEnabled()).toBe(false);
});
it('accepts only the exact enabled flag', () => {
  vi.stubEnv('VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED', 'true');
  expect(topographicEnabled()).toBe(true);
});
it('bounds actual backing allocation, including fractional DPR and huge hosts', () => {
  for (const width of [1, 560, 10000])
    for (const height of [1, 104, 220, 10000])
      for (const dpr of [0.75, 1, 1.5, 4]) {
        const size = boundedViewport(width, height, dpr)!;
        expect(size.width).toBeLessThanOrEqual(560);
        expect(size.height).toBeLessThanOrEqual(220);
        expect(size.ratio).toBeLessThanOrEqual(1.5);
        expect(size.pixelsWide * size.pixelsHigh).toBeLessThanOrEqual(300000);
      }
  for (const bad of [0, -1, Infinity, NaN]) {
    expect(boundedViewport(bad, 10, 1)).toBeNull();
    expect(boundedViewport(10, bad, 1)).toBeNull();
    expect(boundedViewport(10, 10, bad)).toBeNull();
  }
});
it('conservatively declines motion, touch, small screens, Save-Data and absent observers', () => {
  vi.stubGlobal('IntersectionObserver', class {});
  vi.stubGlobal('ResizeObserver', class {});
  vi.stubGlobal(
    'matchMedia',
    vi.fn(() => ({ matches: false })),
  );
  expect(animationAllowed()).toBe(true);
  vi.stubGlobal('innerWidth', 720);
  expect(animationAllowed()).toBe(false);
  vi.stubGlobal('innerWidth', 1440);
  vi.stubGlobal('navigator', { connection: { saveData: true } });
  expect(animationAllowed()).toBe(false);
  vi.stubGlobal('navigator', { connection: { saveData: false } });
  vi.stubGlobal('IntersectionObserver', undefined);
  expect(animationAllowed()).toBe(false);
  vi.stubGlobal('IntersectionObserver', class {});
  vi.stubGlobal('ResizeObserver', undefined);
  expect(animationAllowed()).toBe(false);
  vi.stubGlobal('ResizeObserver', class {});
  for (const blocked of ['reduce', 'coarse']) {
    vi.stubGlobal('matchMedia', (query: string) => ({ matches: query.includes(blocked) }));
    expect(animationAllowed()).toBe(false);
  }
});
it.each(['input', 'textarea', 'select', 'button', 'a', 'div'])('stops for focused %s', (tag) => {
  const node = document.createElement(tag);
  node.setAttribute('tabindex', '0');
  if (tag === 'a') node.setAttribute('href', '#');
  if (tag === 'div') node.setAttribute('contenteditable', 'true');
  document.body.append(node);
  node.focus();
  expect(hasInteractionFocus(document)).toBe(true);
  node.remove();
  expect(hasInteractionFocus(document)).toBe(false);
});
