import { act, cleanup, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AgentMascot } from './AgentMascot';
import type { AgentVisualAction } from './agentVisual';
import { agentMascotAssets } from './agentMascotAssets';

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function fixture() {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance'] });
  const media = Object.assign(new EventTarget(), { matches: false });
  vi.stubGlobal('matchMedia', () => media);
  let visibility!: (entries: { isIntersecting: boolean }[]) => void;
  vi.stubGlobal(
    'IntersectionObserver',
    class {
      constructor(callback: typeof visibility) {
        visibility = callback;
      }
      observe() {}
      disconnect() {}
    },
  );
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(false);
  const scope = {};
  const element = (action: AgentVisualAction, playback = { scope, enabled: true }) => (
    <AgentMascot visual={{ action, label: action }} size={16} playback={playback} />
  );
  const view = render(element('working'));
  return {
    scope,
    show: (action: AgentVisualAction, current = scope, enabled = true) =>
      view.rerender(element(action, { scope: current, enabled })),
    intersect: (...values: boolean[]) =>
      act(() => visibility(values.map((isIntersecting) => ({ isIntersecting })))),
    motion: (matches: boolean) =>
      act(() => {
        media.matches = matches;
        media.dispatchEvent(new Event('change'));
      }),
    hide: (value: boolean) =>
      act(() => {
        hidden.mockReturnValue(value);
        document.dispatchEvent(new Event('visibilitychange'));
      }),
    sprite: () =>
      view.container.querySelector<HTMLImageElement>('.agent-mascot__sprite img:last-child'),
  };
}

const advance = (ms: number) => act(() => vi.advanceTimersByTime(ms));
const cycle = (action: AgentVisualAction) =>
  agentMascotAssets[action].frameDurationsMs.reduce(
    (total: number, duration) => total + duration,
    0,
  );

it('plays acceptance for the observed turn after a static turn-ended poster', () => {
  const player = fixture();
  player.intersect(true);
  player.show('ready', player.scope, false);
  expect(player.sprite()).toBeNull();
  player.show('love');
  advance(0);
  expect(player.sprite()).not.toBeNull();
});

it.each([
  ['working', 2],
  ['failed', 1],
] as const)('plays %s for %i cycles and settles on its poster', (action, repeats) => {
  const player = fixture();
  player.show(action);
  player.intersect(true);
  advance(0);
  expect(player.sprite()).not.toBeNull();
  advance(cycle(action) * repeats - 1);
  expect(player.sprite()).not.toBeNull();
  advance(1);
  expect(player.sprite()).toBeNull();
  expect(vi.getTimerCount()).toBe(0);
});

it('schedules no frames until visible, pauses offscreen and in a hidden tab, and honors motion changes', () => {
  const player = fixture();
  player.intersect(true, false);
  expect(player.sprite()).toBeNull();
  expect(vi.getTimerCount()).toBe(0);
  player.motion(true);
  player.intersect(true);
  expect(vi.getTimerCount()).toBe(0);
  player.motion(false);
  advance(200);
  const frame = player.sprite()?.style.transform;
  expect(frame).toBe('translateX(-32px)');
  for (const setVisible of [(visible: boolean) => player.hide(!visible), player.intersect]) {
    setVisible(false);
    expect(vi.getTimerCount()).toBe(0);
    advance(5000);
    setVisible(true);
    advance(0);
    expect(player.sprite()?.style.transform).toBe(frame);
  }
  player.motion(true);
  expect(player.sprite()).toBeNull();
  expect(vi.getTimerCount()).toBe(0);
});

it.each(['same scope', 'scope round trip'])('ignores repeat completion in %s', (scenario) => {
  const player = fixture();
  player.intersect(true);
  player.show('love');
  advance(0);
  expect(player.sprite()).not.toBeNull();
  player.hide(true);
  player.hide(false);
  advance(cycle('love'));
  expect(player.sprite()).toBeNull();
  player.show('working', scenario === 'same scope' ? player.scope : {});
  player.show('love');
  advance(0);
  expect(player.sprite()).toBeNull();
});
