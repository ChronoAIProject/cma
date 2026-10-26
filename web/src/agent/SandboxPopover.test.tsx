import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { testClient } from '../test/client';
import { AGENT_DETAIL } from '../test/agent';
import { SandboxPopover } from './SandboxPopover';

afterEach(() => vi.useRealTimers());
function draw() {
  const read = vi.fn(async () => ({
    agent_id: AGENT_DETAIL.id,
    sandbox_id: AGENT_DETAIL.sandbox.id,
    uptime_ms: 10000,
    cpu_usage_usec: 1000000,
    cpu_millis_limit: 2000,
    memory_bytes_used: 1024 ** 3,
    memory_bytes_limit: 8 * 1024 ** 3,
    memory_bytes_peak: null,
    stall_permille: null,
    ceiling_hits: null,
    allocation_failures: null,
    kills: null,
    received_bytes: null,
    sent_bytes: null,
  }));
  render(
    <CmaClientProvider client={testClient({ readAgentResources: read })}>
      <textarea aria-label="Message" />
      <SandboxPopover
        detail={AGENT_DETAIL}
        availability={{ state: 'live', runtimeVersion: 'one' }}
      />
    </CmaClientProvider>,
  );
  return read;
}

it('opens on hover without stealing focus and stops resource reads when dismissed', async () => {
  vi.useFakeTimers();
  const read = draw();
  const message = screen.getByRole('textbox', { name: 'Message' });
  message.focus();
  const root = document.querySelector('.agent-toolbar__machine')!;
  fireEvent.pointerEnter(root, { pointerType: 'mouse' });
  await act(async () => {});
  expect(message).toHaveFocus();
  expect(screen.getByRole('dialog', { name: 'Sandbox machine' })).toHaveTextContent(
    AGENT_DETAIL.sandbox.id,
  );
  expect(screen.getByText('RAM · 1 GiB / 8 GiB')).toBeInTheDocument();
  expect(screen.getByRole('region', { name: 'Enabled agent-key scope' })).toHaveTextContent(
    AGENT_DETAIL.reach[0]!,
  );
  expect(read).toHaveBeenCalledOnce();
  fireEvent.pointerLeave(root, { pointerType: 'mouse' });
  await act(() => vi.advanceTimersByTimeAsync(200));
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(read).toHaveBeenCalledOnce();
  expect(message).toHaveFocus();
});

it('supports keyboard and touch activation with Escape dismissal', async () => {
  draw();
  const user = userEvent.setup();
  const button = screen.getByRole('button', { name: 'Sandbox machine' });
  button.focus();
  await user.keyboard('{Enter}');
  expect(screen.getByRole('dialog', { name: 'Sandbox machine' })).toBeInTheDocument();
  await user.keyboard('{Escape}');
  expect(button).toHaveAttribute('aria-expanded', 'false');
  fireEvent.pointerDown(button, { pointerType: 'touch' });
  await user.click(button);
  expect(button).toHaveAttribute('aria-expanded', 'true');
});
