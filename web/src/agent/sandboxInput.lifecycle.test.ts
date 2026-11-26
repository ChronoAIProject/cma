import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { SandboxDetail, SandboxLifecycle, SandboxStatus } from '../api/types';
import { agentClient } from '../test/agent';
import { sandboxFixture, parkedRuntimeResultFixture } from '../test/hierarchy';
import { useSandboxInput } from './sandboxInput';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('keeps a parked parent input-gated and observes it no sooner than five seconds', async () => {
  vi.useFakeTimers();
  const readSandbox = vi.fn(async () => parkedRuntimeResultFixture('revive'));
  const client = agentClient({ readSandbox });
  const { result, unmount } = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1'));
  await act(async () => {});
  expect(result.current.inputAvailability.allowed).toBe(false);
  await act(() => vi.advanceTimersByTimeAsync(4999));
  expect(readSandbox).toHaveBeenCalledTimes(1);
  await act(() => vi.advanceTimersByTimeAsync(1));
  expect(readSandbox).toHaveBeenCalledTimes(2);
  unmount();
});

function transitioning(
  status: SandboxStatus,
  operation?: SandboxLifecycle['operation'],
): SandboxDetail {
  const sandbox = sandboxFixture(status);
  return { ...sandbox, lifecycle: { ...sandbox.lifecycle!, operation } };
}

it.each([
  'provisioning',
  'installing',
  'pausing',
  'reviving',
  'rotating',
  'retiring',
  'retired',
  'failed',
] satisfies SandboxStatus[])(
  'holds messages for %s even without an operation projection',
  async (status) => {
    const client = agentClient({ readSandbox: async () => transitioning(status) });
    const { result } = renderHook(
      () => useSandboxInput(client, 'agt_1', 'sbx_1').inputAvailability,
    );
    await act(async () => {});
    expect(result.current.allowed).toBe(false);
    expect(result.current.notice).toBeTruthy();
  },
);

it('keeps input available during live credential delivery even when Pause is ineligible', async () => {
  const sandbox = transitioning('credential_rotating');
  const client = agentClient({
    readSandbox: async () => ({
      ...sandbox,
      lifecycle: { ...sandbox.lifecycle!, pause: { allowed: false, reason: 'work_in_progress' } },
    }),
  });
  const { result } = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1'));
  await act(async () => {});
  expect(result.current.inputAvailability).toEqual({ allowed: true });
  expect(result.current.resourceAvailability).toEqual({ state: 'live', runtimeVersion: '1.2.3' });
});

it('still holds input when an explicit pause overlaps credential delivery', async () => {
  const client = agentClient({
    readSandbox: async () =>
      transitioning('credential_rotating', {
        purpose: 'pause',
        phase: 'pause_agents',
        startedAt: '2026-09-08T00:00:00Z',
      }),
  });
  const { result } = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1').inputAvailability);
  await act(async () => {});
  expect(result.current.allowed).toBe(false);
  expect(result.current.notice).toContain('Pausing');
});

it.each([
  ['pause', 'capture_and_publish_checkpoint', 'Pausing'],
  ['pause', 'pause_agents', 'Pausing'],
  ['pause', 'teardown_runtime', 'Pausing'],
  ['pause', 'publish_paused', 'Pausing'],
  ['revive', 'home_restore', 'Resuming'],
  ['revive', 'publish_active', 'Resuming'],
  ['image_rotation', 'home_restore', 'Updating runtime'],
  ['image_rotation', 'publish_active', 'Updating runtime'],
] as const)(
  'holds input for %s/%s even while the status still says Active',
  async (purpose, phase, label) => {
    const client = agentClient({
      readSandbox: async () =>
        transitioning('active', {
          purpose,
          phase,
          startedAt: '2026-09-08T00:00:00Z',
        }),
    });
    const { result } = renderHook(
      () => useSandboxInput(client, 'agt_1', 'sbx_1').inputAvailability,
    );
    await act(async () => {});
    expect(result.current.allowed).toBe(false);
    expect(result.current.notice).toContain(label);
  },
);

it.each(['work_in_progress', 'checkpoint_in_progress', 'not_paused', undefined] as const)(
  'holds paused revival when public eligibility refuses it with %s',
  async (reason) => {
    const sandbox = sandboxFixture('paused');
    const client = agentClient({
      readSandbox: async () => ({
        ...sandbox,
        lifecycle: { ...sandbox.lifecycle!, revive: { allowed: false, reason } },
      }),
    });
    const { result } = renderHook(
      () => useSandboxInput(client, 'agt_1', 'sbx_1').inputAvailability,
    );
    await act(async () => {});
    expect(result.current.allowed).toBe(false);
    expect(result.current.notice).toBeTruthy();
  },
);

it.each(['checkpoint_in_progress', 'work_in_progress'] as const)(
  'holds input for the explicitly reported %s blocker without an operation projection',
  async (reason) => {
    const sandbox = sandboxFixture();
    const client = agentClient({
      readSandbox: async () => ({
        ...sandbox,
        lifecycle: { ...sandbox.lifecycle!, pause: { allowed: false, reason } },
      }),
    });
    const { result } = renderHook(
      () => useSandboxInput(client, 'agt_1', 'sbx_1').inputAvailability,
    );
    await act(async () => {});
    expect(result.current.allowed).toBe(false);
    expect(result.current.notice).toBeTruthy();
  },
);

it('does not use unrelated Pause ineligibility as Active message eligibility', async () => {
  const sandbox = sandboxFixture();
  const client = agentClient({
    readSandbox: async () => ({
      ...sandbox,
      lifecycle: { ...sandbox.lifecycle!, pause: { allowed: false, reason: 'not_active' } },
    }),
  });
  const { result } = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1').inputAvailability);
  await act(async () => {});
  expect(result.current).toEqual({ allowed: true });
});

it('follows operator Pause and Resume at the lifecycle cadence and permits only settled states', async () => {
  vi.useFakeTimers();
  const operation = (purpose: 'pause' | 'revive', phase: string) => ({
    purpose,
    phase,
    startedAt: '2026-09-08T00:00:00Z',
  });
  const readSandbox = vi
    .fn()
    .mockResolvedValueOnce(sandboxFixture())
    .mockResolvedValueOnce(
      transitioning('pausing', operation('pause', 'capture_and_publish_checkpoint')),
    )
    .mockResolvedValueOnce(transitioning('pausing', operation('pause', 'teardown_runtime')))
    .mockResolvedValueOnce(sandboxFixture('paused'))
    .mockResolvedValueOnce(transitioning('reviving', operation('revive', 'home_restore')))
    .mockRejectedValueOnce(new Error('read unavailable'))
    .mockResolvedValue(sandboxFixture());
  const client = agentClient({ readSandbox });
  const { result } = renderHook(() => useSandboxInput(client, 'agt_1', 'sbx_1').inputAvailability);
  await act(async () => {});
  expect(result.current.allowed).toBe(true);
  for (const interval of [5000, 1500]) {
    await act(() => vi.advanceTimersByTimeAsync(interval));
    expect(result.current.allowed).toBe(false);
    expect(result.current.notice).toContain('Pausing');
  }
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(result.current).toEqual({ allowed: true });
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(result.current.notice).toContain('Resuming');
  expect(result.current.allowed).toBe(false);
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(result.current.notice).toContain('Resuming');
  expect(result.current.allowed).toBe(false);
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(result.current).toEqual({ allowed: true });
  expect(readSandbox).toHaveBeenCalledTimes(7);
});
