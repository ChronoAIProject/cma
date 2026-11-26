import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { OwnerAgentHeader } from './OwnerAgentHeader';
import { agentTransportJournal, clearAgentTransport } from './transport/journal';
const owner = 'synthetic-owner';
it.each([
  'supported',
  'unsupported',
  'read-failed',
  'other-owner',
  'other-agent',
  'read-only',
  'sign-in',
  'identity-after-menu',
] as const)('keeps the retained prompt inspection entry after close/remount: %s', async (next) => {
  const preferences = {
    title: 'Synthetic',
    system_prompt: 'Original',
    prompt_pending: false,
    prompt_supported: true,
    revision: 3,
  };
  let state: typeof next = 'supported';
  const read = vi.fn(async () => {
    if (state === 'read-failed') throw new TypeError('Synthetic unavailable');
    return { ...preferences, prompt_supported: state === 'supported' };
  });
  const update = vi.fn().mockRejectedValue(new TypeError('Synthetic response lost'));
  const client = agentClient({ readAgentPreferences: read, updateAgentPreferences: update });
  let viewerId = owner;
  let agentId = AGENT_DETAIL.id;
  let readOnly = false;
  const element = () => (
    <CmaClientProvider client={client}>
      <OwnerAgentHeader
        detail={{ ...AGENT_DETAIL, id: agentId }}
        viewerId={viewerId}
        readOnly={readOnly}
        busy={false}
        title="Synthetic"
        status="ready"
        breadcrumb={[]}
        detailsOpen={false}
        onDetailsOpenChange={vi.fn()}
      />
    </CmaClientProvider>
  );
  const view = render(element());
  const open = () => fireEvent.click(screen.getByRole('button', { name: 'More Agent actions' }));
  const item = () => screen.getByRole('menuitem', { name: /^Update system prompt/ });
  open();
  await waitFor(() => expect(item()).toHaveAttribute('aria-disabled', 'false'));
  fireEvent.click(item());
  fireEvent.change(await screen.findByRole('textbox', { name: 'Agent system prompt' }), {
    target: { value: 'Synthetic retained instructions' },
  });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save' })));
  await screen.findByRole('button', { name: 'Retry change' });
  expect(update).toHaveBeenCalledTimes(1);
  const journal = agentTransportJournal(
    sessionStorage,
    owner,
    AGENT_DETAIL.id,
  ).promptSettingsIntent();
  const retained = journal.read();
  expect(retained).toBeDefined();
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  view.unmount();
  state = next;
  if (next === 'other-owner') viewerId = 'other-owner';
  if (next === 'other-agent') agentId = 'other-agent';
  if (next === 'read-only') readOnly = true;
  if (next === 'sign-in') {
    clearAgentTransport(sessionStorage);
    agentTransportJournal(sessionStorage, owner, AGENT_DETAIL.id);
  }
  const before = JSON.stringify({ ...sessionStorage });
  render(element());
  open();
  await waitFor(() =>
    expect(item()).not.toHaveAccessibleName(
      expect.stringContaining('Checking system prompt support'),
    ),
  );
  if (next === 'identity-after-menu') {
    expect(item()).toHaveAttribute('aria-disabled', 'false');
    clearAgentTransport(sessionStorage);
    agentTransportJournal(sessionStorage, 'other-owner', AGENT_DETAIL.id);
    const changedIdentity = JSON.stringify({ ...sessionStorage });
    fireEvent.click(item());
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(JSON.stringify({ ...sessionStorage })).toBe(changedIdentity);
    expect(update).toHaveBeenCalledTimes(1);
    return;
  }
  if (
    next === 'other-owner' ||
    next === 'other-agent' ||
    next === 'read-only' ||
    next === 'sign-in'
  ) {
    expect(item()).toHaveAttribute('aria-disabled', 'true');
    fireEvent.click(item());
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(update).toHaveBeenCalledTimes(1);
    expect(JSON.stringify({ ...sessionStorage })).toBe(before);
    return;
  }
  expect(JSON.stringify(journal.read())).toBe(JSON.stringify(retained));
  expect(update).toHaveBeenCalledTimes(1);
  expect(item()).toHaveAttribute('aria-disabled', 'false');
  fireEvent.click(item());
  expect(await screen.findByRole('dialog', { name: 'Update system prompt' })).toBeVisible();
  if (next === 'read-failed') await screen.findByRole('button', { name: 'Reload preferences' });
  else
    await screen.findByText(
      'Re-enter the exact original system prompt to check the pending change.',
    );
  expect(update).toHaveBeenCalledTimes(1);
});
