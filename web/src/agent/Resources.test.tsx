import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AgentResources } from '@chronoai/cma-sdk';
import { CmaClientProvider } from '../api/context';
import { testClient } from '../test/client';
import { AGENT_USAGE } from '../test/agent';
import { Resources, cpuMillis, formatBytes } from './Resources';

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
      <Resources
        agentId="agt_1"
        sandboxId="sbx_1"
        usage={{ ...AGENT_USAGE, contextWindow: 10000 }}
      />
    </CmaClientProvider>,
  );
  await act(async () => {
    await Promise.resolve();
  });
  expect(screen.getByText('CPU · measuring')).toBeInTheDocument();
  expect(screen.getByText('RAM · 1 GiB / 8 GiB')).toBeInTheDocument();
  expect(screen.getByText('Network · ↓ 2 KiB · ↑ 1 KiB')).toBeInTheDocument();
  expect(screen.getByRole('progressbar', { name: 'Context' })).toHaveAttribute(
    'aria-valuemax',
    '10000',
  );
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
      <Resources agentId="agt_1" sandboxId="sbx_1" />
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
  expect(screen.getByText('Context · not reported')).toBeInTheDocument();
});

it('never displays a resource response for a different Agent', async () => {
  render(
    <CmaClientProvider
      client={testClient({
        readAgentResources: vi.fn().mockResolvedValue({ ...reading, agent_id: 'other' }),
      })}
    >
      <Resources agentId="agt_1" sandboxId="sbx_1" />
    </CmaClientProvider>,
  );
  expect(await screen.findByText('Resources unavailable')).toBeInTheDocument();
  expect(screen.queryByText('RAM · 1 GiB / 8 GiB')).not.toBeInTheDocument();
});
