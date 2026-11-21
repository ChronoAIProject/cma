import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { WherePicker } from './WherePicker';

afterEach(() => vi.useRealTimers());

it('explains the location fields and excludes deleting/deleted choices', async () => {
  const actor = userEvent.setup();
  const workspace = workspaceFixture();
  const sandbox = {
    ...sandboxFixture(),
    workspaces: [
      workspace,
      {
        ...workspace,
        id: 'removed-workspace',
        name: 'Deleted workspace',
        status: 'removed' as const,
      },
      {
        ...workspace,
        id: 'removing-workspace',
        name: 'Deleting workspace',
        status: 'removing' as const,
      },
    ],
  };
  render(
    <WherePicker
      client={agentClient({
        listSandboxes: async () => ({
          items: [
            sandbox,
            { ...sandboxFixture('retired'), id: 'retired-sandbox', name: 'Deleted sandbox' },
            { ...sandboxFixture('retiring'), id: 'retiring-sandbox', name: 'Deleting sandbox' },
          ],
        }),
        readSandbox: async () => sandbox,
      })}
      value={{ sandboxId: sandbox.id, workspaceId: workspace.id }}
      onChange={vi.fn()}
      onReady={vi.fn()}
    />,
  );
  expect(screen.getByText('Sandbox environment')).toBeInTheDocument();
  expect(screen.getByText('Project workspace')).toBeInTheDocument();
  await actor.click(await screen.findByRole('button', { name: 'Sandbox' }));
  expect(screen.getByRole('menuitemradio', { name: 'Atlas' })).toBeInTheDocument();
  expect(
    screen.queryByRole('menuitemradio', { name: /Deleted sandbox|Deleting sandbox/ }),
  ).not.toBeInTheDocument();
  await actor.keyboard('{Escape}');
  await actor.click(screen.getByRole('button', { name: 'Workspace' }));
  expect(screen.getByRole('menuitemradio', { name: 'chrono-bot' })).toBeInTheDocument();
  expect(
    screen.queryByRole('menuitemradio', { name: /Deleted workspace|Deleting workspace/ }),
  ).not.toBeInTheDocument();
});

it('revokes readiness and removes a selected Workspace after observed deletion', async () => {
  vi.useFakeTimers();
  const workspace = workspaceFixture();
  const onReady = vi.fn();
  let removed = false;
  render(
    <WherePicker
      client={agentClient({
        readWorkspace: async () => ({ ...workspace, status: removed ? 'removed' : 'ready' }),
      })}
      value={{ sandboxId: 'sbx_1', workspaceId: workspace.id }}
      onChange={vi.fn()}
      onReady={onReady}
    />,
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(onReady).toHaveBeenLastCalledWith(true);
  removed = true;
  await act(() => vi.advanceTimersByTimeAsync(5000));
  expect(onReady).toHaveBeenLastCalledWith(false);
  expect(screen.getByRole('button', { name: 'Workspace' })).not.toHaveTextContent('chrono-bot');
});
