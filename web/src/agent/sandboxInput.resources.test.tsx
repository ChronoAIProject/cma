import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import type { SandboxDetail } from '../api/types';
import { useSandboxInput } from './sandboxInput';

afterEach(() => vi.useRealTimers());

it.each([
  ['pausing', 'Pausing'],
  ['reviving', 'Resuming'],
  ['paused', 'Sandbox paused'],
] as const)(
  'reports authoritative %s without treating initialized Agents as live',
  async (status, notice) => {
    const client = agentClient({ readSandbox: async () => sandboxFixture(status) });
    const view = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1'));
    await act(async () => {});
    expect(view.result.current.resourceAvailability).toMatchObject({
      state: status === 'paused' ? 'paused' : 'inactive',
      notice: expect.stringContaining(notice),
    });
  },
);

it.each(['active', 'paused'] as const)(
  'holds resources while a %s Sandbox has an operation',
  async (status) => {
    const sandbox = sandboxFixture(status);
    const client = agentClient({
      readSandbox: async () => ({
        ...sandbox,
        lifecycle: {
          ...sandbox.lifecycle!,
          operation: {
            purpose: 'revive',
            phase: 'runtime_ready',
            startedAt: '2026-11-01T00:00:00Z',
          },
        },
      }),
    });
    const view = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1'));
    await act(async () => {});
    expect(view.result.current.resourceAvailability).toMatchObject({
      state: 'inactive',
      notice: expect.stringContaining('Resuming'),
    });
  },
);

it('withdraws expired pause evidence during outage and requires a fresh Active observation', async () => {
  vi.useFakeTimers();
  let sandbox: SandboxDetail = sandboxFixture('paused');
  let outage = false;
  const client = agentClient({
    readSandbox: async () => {
      if (outage) throw new Error('offline');
      return sandbox;
    },
  });
  const view = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1'));
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(view.result.current.resourceAvailability.state).toBe('paused');
  outage = true;
  await act(() => vi.advanceTimersByTimeAsync(15_001));
  expect(view.result.current.resourceAvailability.state).toBe('unavailable');
  sandbox = sandboxFixture('active');
  outage = false;
  await act(() => vi.advanceTimersByTimeAsync(5_000));
  expect(view.result.current.resourceAvailability).toEqual({
    state: 'live',
    runtimeVersion: sandbox.runtimeVersion,
  });
});
