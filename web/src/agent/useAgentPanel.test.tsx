import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useAgentPanel } from './useAgentPanel';

afterEach(() => vi.restoreAllMocks());

function viewport(wide: boolean) {
  const query = new EventTarget() as MediaQueryList;
  Object.defineProperty(query, 'matches', { configurable: true, value: wide });
  vi.spyOn(window, 'matchMedia').mockReturnValue(query);
  return (next: boolean) => {
    Object.defineProperty(query, 'matches', { configurable: true, value: next });
    act(() => query.dispatchEvent(new Event('change')));
  };
}

it.each([false, true])('starts closed and never opens on resize (wide=%s)', (wide) => {
  const resize = viewport(wide);
  const { result, rerender } = renderHook(() => useAgentPanel('owner', 'agent-one'));
  expect(result.current.panel).toBeUndefined();
  resize(!wide);
  resize(wide);
  rerender();
  expect(result.current.panel).toBeUndefined();
});

it('retains explicit choices within one conversation, including closing after a tool jump', () => {
  const resize = viewport(true);
  const { result, rerender } = renderHook(() => useAgentPanel('owner', 'agent-one'));
  act(() => result.current.setPanel('browser'));
  const select = result.current.setPanel;
  rerender();
  resize(false);
  expect(result.current.panel).toBe('browser');
  expect(result.current.setPanel).toBe(select);
  act(() => result.current.setDetailsOpen(true));
  expect(result.current.detailsOpen).toBe(true);
  act(() => result.current.setPanel('activity'));
  expect(result.current.panel).toBe('activity');
  act(() => result.current.setPanel(undefined));
  resize(true);
  rerender();
  expect(result.current.panel).toBeUndefined();
});

it.each([
  { viewer: 'owner', agent: 'agent-two' },
  { viewer: 'other-owner', agent: 'agent-one' },
  { viewer: 'owner', agent: undefined },
])('closes on scope change to $viewer/$agent and ignores a previous viewer callback', (next) => {
  viewport(true);
  const initial = { viewer: 'owner', agent: 'agent-one' as string | undefined };
  const { result, rerender } = renderHook(({ viewer, agent }) => useAgentPanel(viewer, agent), {
    initialProps: initial,
  });
  act(() => result.current.setPanel('files'));
  const oldSelection = result.current.setPanel;
  rerender(next);
  expect(result.current.panel).toBeUndefined();
  act(() => result.current.setPanel('browser'));
  act(() => oldSelection('activity'));
  expect(result.current.panel).toBe('browser');
  rerender(initial);
  expect(result.current.panel).toBeUndefined();
});

it('keeps the newest tab when an earlier open or automatic fallback is applied', () => {
  const { result } = renderHook(() => useAgentPanel('owner', 'agent-one'));
  const { setComputerOpen, setPanel } = result.current;
  act(() => setComputerOpen(true));
  expect(result.current.panel).toBe('browser');
  act(() => setPanel('files'));
  act(() => setComputerOpen(true));
  act(() => setPanel('browser', 'activity'));
  expect(result.current.panel).toBe('files');

  act(() => setPanel('activity'));
  act(() => setPanel('browser', 'activity'));
  expect(result.current.panel).toBe('browser');

  act(() => setComputerOpen(false));
  expect(result.current.panel).toBeUndefined();
  act(() => setComputerOpen(true));
  expect(result.current.panel).toBe('browser');
});
