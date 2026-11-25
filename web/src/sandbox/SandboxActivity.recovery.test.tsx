import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ProtocolError } from '@cma/client';
import { createRestWebClient } from '../api/rest';
import { restTransport } from '../test/rest';
import { conversation, proof, receipt } from '../test/runtimeRecovery';
import { SandboxActivity } from './SandboxActivity';

const at = '2026-11-01T00:00:00Z';
const recovery = [
  {
    type: 'runtime_recovery_admitted',
    previous_status: 'failed',
    target_runtime_version: 'runtime-next',
    proof,
  },
  { type: 'runtime_recovery_quiesced', receipt },
  {
    type: 'work_step_failed',
    step: { kind: 'run_lifecycle_operation' },
    failure: { class: 'temporary', detail: 'private failure' },
  },
  { type: 'work_step_completed', step: { kind: 'run_lifecycle_operation' } },
  { type: 'status_changed', from: 'rotating', to: 'active' },
  { type: 'rotated', runtime_version: 'runtime-next' },
  {
    type: 'home_restored',
    restore_id: 'restore-fixture',
    backup_id: 'backup-fixture',
    restored_to: at,
  },
];
const page = recovery.map((kind, index) => ({
  sandbox_id: 'sbx_1',
  seq: index + 1,
  appended_at: at,
  kind,
}));

afterEach(() => vi.useRealTimers());

it('reads retained recovery evidence without newer reservation fields or a boot identity', async () => {
  const older = Object.fromEntries(
    Object.entries(proof).filter(
      ([key]) => key !== 'reserved_provision' && key !== 'observed_provision_generation',
    ),
  );
  const retained = [
    { ...page[0], kind: { ...recovery[0], proof: { ...older, boot_id: null, conversations: [] } } },
    {
      ...page[1],
      kind: { ...recovery[1], receipt: { ...receipt, boot_id: null, conversations: [] } },
    },
  ];
  const transport = restTransport();
  transport.reply(retained);
  expect(await transport.sdk.sandboxes.events('sbx_1')).toEqual(retained);
});

it('reads the complete recovery history through the real decoder after every remount', async () => {
  const transport = restTransport();
  const client = createRestWebClient(transport.sdk);
  for (let attempt = 0; attempt < 2; attempt += 1) {
    transport.reply(page);
    transport.reply([]);
    const view = render(<SandboxActivity client={client} sandboxId="sbx_1" settled />);
    expect(await screen.findByText('Runtime recovery requested')).toBeVisible();
    expect(screen.getByText('Runtime recovery prepared')).toBeVisible();
    expect(screen.getByText('Sandbox operation stage: needs attention')).toBeVisible();
    expect(screen.getByText('Sandbox updated')).toBeVisible();
    expect(screen.getByText('Sandbox home restored')).toBeVisible();
    expect(screen.getAllByRole('listitem')).toHaveLength(7);
    expect(screen.queryByText('No activity recorded yet.')).not.toBeInTheDocument();
    expect(view.container).not.toHaveTextContent(
      /private|runtime-fixture|home-fixture|conversation-fixture/,
    );
    view.unmount();
  }
});

it.each([
  { ...recovery[0], proof: null },
  { ...recovery[0], proof: { ...proof, runtime_generation: -1 } },
  { ...recovery[0], proof: { ...proof, provider_plan: 'invalid' } },
  { ...recovery[0], proof: { ...proof, conversations: [{ ...conversation, history_sha256: 1 }] } },
  { ...recovery[1], receipt: { ...receipt, runtime_process_absent: 'true' } },
  { type: 'unknown_recovery_event' },
])('rejects malformed recovery evidence without accepting a partial history', async (kind) => {
  const transport = restTransport();
  transport.reply([page[0], { ...page[1], kind }]);
  await expect(transport.sdk.sandboxes.events('sbx_1')).rejects.toBeInstanceOf(ProtocolError);
});

it('keeps valid activity visible when a later recovery page is malformed', async () => {
  vi.useFakeTimers();
  const transport = restTransport();
  transport.reply(page);
  transport.reply([]);
  transport.reply([
    { ...page[0], seq: 8, kind: { type: 'runtime_recovery_quiesced', receipt: null } },
  ]);
  render(<SandboxActivity client={createRestWebClient(transport.sdk)} sandboxId="sbx_1" />);
  await act(async () => {});
  expect(screen.getByText('Sandbox updated')).toBeVisible();
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(screen.getByText('Activity updates interrupted. Reconnecting.')).toBeVisible();
  expect(screen.getAllByRole('listitem')).toHaveLength(7);
});
