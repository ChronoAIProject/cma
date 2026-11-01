import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AgentResources } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import { testClient } from '../test/client';
import { Resources, cpuMillis, formatBytes } from './Resources';
import type { SandboxResourceAvailability } from './sandboxInput';

const liveAvailability = { state: 'live', runtimeVersion: 'runtime-one' } as const;

const reading: AgentResources = {
  agent_id: 'agt_1',
  sandbox_id: 'sbx_1',
  uptime_ms: 10000,
  cpu_usage_usec: 1000000,
  cpu_millis_limit: 4000,
  memory_bytes_used: 1024 ** 3,
  memory_bytes_limit: 8 * 1024 ** 3,
  memory_bytes_peak: null,
  stall_permille: null,
  ceiling_hits: null,
  allocation_failures: null,
  kills: null,
  received_bytes: 2048,
  sent_bytes: 1024,
};
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('cancels reads on pause, ignores late samples, and starts CPU measurement again on activation or image change', async () => {
  vi.useFakeTimers();
  let finish: (value: AgentResources) => void = () => {};
  const read = vi
    .fn()
    .mockResolvedValueOnce(reading)
    .mockImplementationOnce(
      () =>
        new Promise<AgentResources>((resolve) => {
          finish = resolve;
        }),
    )
    .mockResolvedValueOnce({ ...reading, uptime_ms: 20000, cpu_usage_usec: 11000000 })
    .mockResolvedValue({ ...reading, uptime_ms: 25000, cpu_usage_usec: 21000000 });
  const client = testClient({ readAgentResources: read });
  const draw = (availability: SandboxResourceAvailability) => (
    <CmaClientProvider client={client}>
      <Resources agentId="agt_1" sandboxId="sbx_1" availability={availability} />
    </CmaClientProvider>
  );
  const { rerender } = render(draw(liveAvailability));
  await act(async () => {});
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(read).toHaveBeenCalledTimes(2);
  rerender(
    draw({ state: 'paused', notice: 'Sandbox paused. Live resources resume when it is active.' }),
  );
  expect(read.mock.calls[1]?.[1].signal.aborted).toBe(true);
  await act(async () => finish({ ...reading, memory_bytes_used: 3 * 1024 ** 3 }));
  await act(() => vi.advanceTimersByTimeAsync(15000));
  expect(read).toHaveBeenCalledTimes(2);
  expect(screen.getByText('RAM · paused')).toBeInTheDocument();
  expect(screen.queryByRole('progressbar', { name: 'RAM' })).not.toBeInTheDocument();
  expect(screen.queryByRole('progressbar', { name: 'Context' })).not.toBeInTheDocument();
  rerender(draw(liveAvailability));
  await act(async () => {});
  expect(read).toHaveBeenCalledTimes(3);
  expect(screen.getByText('CPU · measuring')).toBeInTheDocument();
  expect(screen.getByText('RAM · 1 GiB / 8 GiB')).toBeInTheDocument();
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(screen.getByText('CPU · 2 cores / 4 cores')).toBeInTheDocument();
  rerender(draw({ state: 'live', runtimeVersion: 'runtime-two' }));
  await act(async () => {});
  expect(read).toHaveBeenCalledTimes(5);
  expect(screen.getByText('CPU · measuring')).toBeInTheDocument();
});

it('computes CPU cores only from comparable monotonic Sandbox samples', () => {
  expect(cpuMillis(undefined, reading)).toBeUndefined();
  expect(cpuMillis(reading, { ...reading, uptime_ms: 15000, cpu_usage_usec: 11000000 })).toBe(2000);
  expect(cpuMillis(reading, { ...reading, uptime_ms: 9000 })).toBeUndefined();
  expect(cpuMillis(reading, { ...reading, uptime_ms: 15000, cpu_usage_usec: 5 })).toBeUndefined();
  expect(cpuMillis(reading, { ...reading, sandbox_id: 'other' })).toBeUndefined();
  expect(cpuMillis(reading, { ...reading, cpu_usage_usec: null })).toBeUndefined();
  expect(formatBytes(0)).toBe('0 B');
  expect(formatBytes(1536)).toBe('1.5 KiB');
});

it('renders measured memory, network and context and derives live CPU on the next read', async () => {
  vi.useFakeTimers();
  const read = vi
    .fn()
    .mockResolvedValueOnce(reading)
    .mockResolvedValue({ ...reading, uptime_ms: 15000, cpu_usage_usec: 11000000 });
  const view = render(
    <CmaClientProvider client={testClient({ readAgentResources: read })}>
      <Resources availability={liveAvailability} agentId="agt_1" sandboxId="sbx_1" />
    </CmaClientProvider>,
  );
  await act(async () => {
    await Promise.resolve();
  });
  expect(screen.getByText('CPU · measuring')).toBeInTheDocument();
  expect(screen.getByText('RAM · 1 GiB / 8 GiB')).toBeInTheDocument();
  expect(screen.getByText('Network · ↓ 2 KiB · ↑ 1 KiB')).toBeInTheDocument();
  expect(screen.queryByRole('progressbar', { name: 'Context' })).not.toBeInTheDocument();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  expect(screen.getByText('CPU · 2 cores / 4 cores')).toBeInTheDocument();
  expect(screen.getByRole('progressbar', { name: 'CPU' })).toHaveAttribute('aria-valuenow', '2000');
  view.unmount();
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  expect(read).toHaveBeenCalledTimes(2);
});

it('retains last readings and explicitly marks a failed refresh', async () => {
  vi.useFakeTimers();
  const read = vi.fn().mockResolvedValueOnce(reading).mockRejectedValue(new Error('offline'));
  render(
    <CmaClientProvider client={testClient({ readAgentResources: read })}>
      <Resources agentId="agt_1" sandboxId="sbx_1" availability={liveAvailability} />
    </CmaClientProvider>,
  );
  await act(async () => {
    await Promise.resolve();
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(5000);
  });
  expect(screen.getByText('RAM · 1 GiB / 8 GiB')).toBeInTheDocument();
  expect(screen.getByRole('status')).toHaveTextContent('Last reading; reconnecting');
  expect(screen.queryByText('Context · not reported')).not.toBeInTheDocument();
});

it('never displays a resource response for a different Agent', async () => {
  render(
    <CmaClientProvider
      client={testClient({
        readAgentResources: vi.fn().mockResolvedValue({ ...reading, agent_id: 'other' }),
      })}
    >
      <Resources agentId="agt_1" sandboxId="sbx_1" availability={liveAvailability} />
    </CmaClientProvider>,
  );
  expect(
    await screen.findByText('Usage readings are unavailable right now. Your Agent keeps working.'),
  ).toBeInTheDocument();
  expect(screen.queryByText('RAM · 1 GiB / 8 GiB')).not.toBeInTheDocument();
});

it.each([null, 4000])(
  'labels configured capacity per metric and prefers observed limits (%s)',
  async (cpuLimit) => {
    vi.useFakeTimers();
    const first = { ...reading, memory_bytes_limit: null, cpu_millis_limit: cpuLimit };
    const read = vi
      .fn()
      .mockResolvedValueOnce(first)
      .mockResolvedValue({
        ...first,
        uptime_ms: 15000,
        cpu_usage_usec: 11000000,
      });
    const readCapacity = vi
      .fn()
      .mockResolvedValue({ cpuMillis: 8000, memoryBytes: 16 * 1024 ** 3 });
    const client = testClient({
      readAgentResources: read,
      readSandboxProfileCapacity: readCapacity,
    });
    const draw = () => (
      <CmaClientProvider client={client}>
        <Resources
          availability={liveAvailability}
          agentId="agt_1"
          sandboxId="sbx_1"
          sandboxProfile={{ id: 'profile', revision: 2, digest: 'digest' }}
        />
      </CmaClientProvider>
    );
    const { rerender } = render(draw());
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000);
    });
    expect(screen.getByText('RAM · 1 GiB / 16 GiB configured')).toBeInTheDocument();
    expect(screen.getByRole('progressbar', { name: 'RAM' })).toHaveAttribute(
      'aria-valuetext',
      '1 GiB of 16 GiB configured',
    );
    expect(screen.getByRole('progressbar', { name: 'CPU' })).toHaveAttribute(
      'aria-valuetext',
      cpuLimit === null ? '2 cores of 8 cores configured' : '2 cores of 4 cores',
    );
    rerender(draw());
    expect(readCapacity).toHaveBeenCalledOnce();
  },
);

it('leaves absolute readings without bars when pinned capacity cannot be read', async () => {
  render(
    <CmaClientProvider
      client={testClient({
        readAgentResources: vi
          .fn()
          .mockResolvedValue({ ...reading, memory_bytes_limit: null, cpu_millis_limit: null }),
        readSandboxProfileCapacity: vi.fn().mockRejectedValue(new Error('forbidden')),
      })}
    >
      <Resources
        availability={liveAvailability}
        agentId="agt_1"
        sandboxId="sbx_1"
        sandboxProfile={{ id: 'profile', revision: 1 }}
      />
    </CmaClientProvider>,
  );
  expect(await screen.findByText('RAM · 1 GiB')).toBeInTheDocument();
  expect(screen.queryByRole('progressbar', { name: 'RAM' })).not.toBeInTheDocument();
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
});

it('ignores an old profile response and never carries configured capacity into another Agent', async () => {
  let resolveOld: ((value: { cpuMillis: number; memoryBytes: number }) => void) | undefined;
  const readCapacity = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveOld = resolve;
        }),
    )
    .mockRejectedValue(new Error('not found'));
  const client = testClient({
    readAgentResources: vi.fn().mockImplementation(async (id: string) => ({
      ...reading,
      agent_id: id,
      memory_bytes_limit: null,
      cpu_millis_limit: null,
    })),
    readSandboxProfileCapacity: readCapacity,
  });
  const draw = (agentId: string, revision: number) => (
    <CmaClientProvider client={client}>
      <Resources
        agentId={agentId}
        sandboxId="sbx_1"
        sandboxProfile={{ id: 'profile', revision }}
        availability={liveAvailability}
      />
    </CmaClientProvider>
  );
  const { rerender } = render(draw('agt_1', 1));
  await act(async () => {
    await Promise.resolve();
  });
  rerender(draw('agt_2', 2));
  await act(async () => {
    resolveOld?.({ cpuMillis: 8000, memoryBytes: 16 * 1024 ** 3 });
  });
  expect(screen.getByText('RAM · 1 GiB')).toBeInTheDocument();
  expect(screen.queryByRole('progressbar', { name: 'RAM' })).not.toBeInTheDocument();
  expect(readCapacity.mock.calls[0]?.[1].signal.aborted).toBe(true);
});
