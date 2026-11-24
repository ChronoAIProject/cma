import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { SandboxUsageEvent } from '@cma/client';
import { expect, it, vi } from 'vitest';
import { UsageHistory } from './UsageHistory';

const allocation = { running: false, cpu_millis: 0, memory_mib: 0, disk_gib: 0 };
const checkpoint: SandboxUsageEvent = {
  transaction_id: 'cma-usage:retired-sandbox:7',
  revision: 7,
  source_seq: 4,
  kind: 'checkpoint',
  started_at: 1_700_000_000,
  ended_at: 1_700_000_000,
  status: 'retired',
  allocation,
  next_allocation: allocation,
  quantity: {
    uptime_seconds: 0,
    cpu_core_hours: 0,
    memory_gib_hours: 0,
    disk_gib_hours: 0,
    cpu_millis_seconds: 0,
    memory_mib_seconds: 0,
    disk_gib_seconds: 0,
  },
};

it('shows each retired Sandbox transaction once and distinguishes identical-looking checkpoints on repeated reads', async () => {
  const nextCheckpoint = {
    ...checkpoint,
    transaction_id: 'cma-usage:retired-sandbox:8',
    revision: 8,
  };
  const retirement = {
    ...checkpoint,
    transaction_id: 'cma-usage:retired-sandbox:9',
    revision: 9,
    kind: 'retired',
  };
  const events = vi.fn(async (_id: string, { cursor }: { cursor?: string }) =>
    cursor === 'page-two'
      ? { items: [retirement], next: null }
      : { items: [checkpoint, { ...checkpoint }, nextCheckpoint], next: 'page-two' },
  );
  const actor = userEvent.setup();
  render(<UsageHistory client={{ events }} id="retired-sandbox" onClose={vi.fn()} />);
  const history = within(screen.getByRole('region', { name: 'Sandbox usage history' }));
  const assertFirstPage = async () => {
    const table = within(await history.findByRole('table', { name: 'Usage intervals' }));
    expect(
      table
        .getAllByRole('row')
        .slice(1)
        .map((row) => within(row).getAllByRole('cell')[1]?.textContent),
    ).toEqual(['7', '8']);
  };

  await history.findByRole('button', { name: 'Next history page' });
  await assertFirstPage();
  await actor.click(history.getByRole('button', { name: 'Next history page' }));
  expect(await history.findByRole('cell', { name: '9' })).toBeVisible();
  await actor.click(history.getByRole('button', { name: 'First history page' }));
  await history.findByRole('button', { name: 'Next history page' });
  await assertFirstPage();
  expect(events.mock.calls.map(([, options]) => options.cursor)).toEqual([
    undefined,
    'page-two',
    undefined,
  ]);
});
