import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { DeletionDialog } from './DeletionDialog';
import { DeletionImpactSummary } from './DeletionImpactSummary';
import { DeletionStatusPanel } from './DeletionStatusPanel';
import type { DeletionObservation, DeletionRequest } from './deletion';
import { agentClient } from './test/agent';
import { sandboxFixture, workspaceFixture } from './test/hierarchy';
import type { WorkspaceDetail } from './api/types';

const request: DeletionRequest = {
  action: { kind: 'delete_sandbox', sandboxId: 'sbx_1' },
  key: 'synthetic-delete-reference',
  accepted: false,
};
const observation: DeletionObservation = {
  name: 'Example environment',
  complete: false,
  observedAt: '2026-11-22T00:00:00Z',
  events: [],
  steps: [{ label: 'Stop Agents', status: 'In progress' }],
};
const clipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
afterEach(() => {
  vi.useRealTimers();
  localStorage.clear();
  if (clipboard) Object.defineProperty(navigator, 'clipboard', clipboard);
  else Reflect.deleteProperty(navigator, 'clipboard');
});

it('shows known impact, keeps unchecked deletion inert, and allows dismissal during checks', async () => {
  const deleteSandbox = vi.fn();
  const close = vi.fn();
  const view = render(
    <DeletionDialog
      viewerId="owner"
      client={agentClient({ deleteSandbox })}
      initial={request}
      restored={false}
      onChanged={vi.fn()}
      onClose={close}
    />,
  );
  expect(screen.getByRole('button', { name: 'Keep environment' })).toBeEnabled();
  await screen.findByText('Agent one');
  expect(screen.getByRole('list', { name: 'Known affected resources' })).toHaveTextContent(
    'chrono-bot',
  );
  expect(screen.getByText(/full impact may not be listed/)).toBeVisible();
  expect(screen.getByRole('checkbox')).not.toBeChecked();
  fireEvent.click(screen.getByRole('button', { name: 'Request deletion' }));
  expect(deleteSandbox).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Keep environment' }));
  expect(close).toHaveBeenCalledOnce();
  expect(view.container.querySelector('p p, p div, p ul')).toBeNull();
});

it.each(['delete_agent', 'remove_workspace'] as const)(
  '%s never claims a Sandbox slot will be released',
  async (kind) => {
    const action =
      kind === 'delete_agent' ? { kind, agentId: 'agt_1' } : { kind, workspaceId: 'wsp_1' };
    render(
      <DeletionImpactSummary
        client={agentClient()}
        viewerId="owner"
        action={action}
        observation={{ ...observation, sandboxId: 'sbx_1' }}
        available
      />,
    );
    if (kind === 'remove_workspace') await screen.findByText('Agent one');
    expect(screen.queryByText(/slot frees up/)).not.toBeInTheDocument();
    expect(
      screen.getByText(kind === 'delete_agent' ? /still counts toward/ : /does not release/),
    ).toBeVisible();
  },
);

it.each([403, 404, 503])(
  'does not report an empty impact list for a %s read failure',
  async (status) => {
    render(
      <DeletionImpactSummary
        client={agentClient({
          readSandbox: async () => {
            throw new ApiError(status, 'Synthetic unavailable');
          },
        })}
        viewerId="owner"
        action={request.action}
        observation={observation}
        available
      />,
    );
    expect(await screen.findByText(/This is not an empty environment/)).toBeVisible();
    expect(screen.queryByText('Agent one')).not.toBeInTheDocument();
  },
);

it('keeps Workspace summaries when a child read fails and bounds the preview reads', async () => {
  const sandbox = sandboxFixture();
  const workspaces = Array.from({ length: 12 }, (_, index) => ({
    ...workspaceFixture(),
    id: `wsp_${index}`,
    name: `Workspace ${index}`,
  }));
  const readWorkspace = vi.fn(async () => {
    throw new ApiError(403, 'Synthetic denied');
  });
  render(
    <DeletionImpactSummary
      client={agentClient({
        readSandbox: async () => ({ ...sandbox, workspaces, workspaceCount: workspaces.length }),
        readWorkspace,
      })}
      viewerId="owner"
      action={request.action}
      observation={observation}
      available
    />,
  );
  await screen.findByText('Workspace 7');
  expect(readWorkspace).toHaveBeenCalledTimes(8);
  expect(screen.queryByText('Workspace 8')).not.toBeInTheDocument();
  expect(screen.getByText(/full impact may not be listed/)).toBeVisible();
});

it('rejects mismatched Sandbox and child identities without disclosing foreign names', async () => {
  const readWorkspace = vi.fn(async () => ({
    ...workspaceFixture(),
    id: 'wsp_foreign',
    name: 'Foreign workspace',
  }));
  const client = agentClient({ readWorkspace });
  const view = render(
    <DeletionImpactSummary
      client={client}
      viewerId="owner"
      action={request.action}
      observation={observation}
      available
    />,
  );
  await screen.findByText(/full impact may not be listed/);
  expect(screen.queryByText('Agent one')).not.toBeInTheDocument();
  expect(screen.queryByText('Foreign workspace')).not.toBeInTheDocument();
  view.rerender(
    <DeletionImpactSummary
      client={agentClient({
        readSandbox: async () => ({
          ...sandboxFixture(),
          id: 'sbx_foreign',
          name: 'Foreign environment',
        }),
        readWorkspace,
      })}
      viewerId="owner"
      action={request.action}
      observation={observation}
      available
    />,
  );
  await screen.findByText(/This is not an empty environment/);
  expect(screen.queryByText('Foreign environment')).not.toBeInTheDocument();
  expect(readWorkspace).toHaveBeenCalledTimes(1);
});

it.each(['identity', 'target', 'unmount', 'unavailable'] as const)(
  'aborts impact reads and ignores delayed results after %s changes',
  async (change) => {
    let resolve!: (value: WorkspaceDetail) => void;
    let signal: AbortSignal | undefined;
    const readWorkspace = vi.fn((_id, options) => {
      signal = options?.signal;
      return new Promise<WorkspaceDetail>((done) => {
        resolve = done;
      });
    });
    const client = agentClient({ readWorkspace });
    const props = {
      client,
      viewerId: 'owner',
      action: request.action,
      observation,
      available: true,
    };
    const view = render(<DeletionImpactSummary {...props} />);
    await waitFor(() => expect(readWorkspace).toHaveBeenCalledOnce());
    const previousSignal = signal;
    if (change === 'unmount') view.unmount();
    else
      view.rerender(
        <DeletionImpactSummary
          {...props}
          viewerId={change === 'identity' ? 'other' : 'owner'}
          action={
            change === 'target' ? { kind: 'delete_agent', agentId: 'agt_other' } : request.action
          }
          available={change !== 'unavailable'}
        />,
      );
    expect(previousSignal?.aborted).toBe(true);
    await act(async () =>
      resolve({
        ...workspaceFixture(),
        agents: [{ ...workspaceFixture().agents[0]!, title: 'Delayed private title' }],
      }),
    );
    expect(screen.queryByText('Delayed private title')).not.toBeInTheDocument();
  },
);

it.each([
  [false, {}, undefined, 'Request unconfirmed'],
  [true, {}, undefined, 'Deletion requested'],
  [true, { inProgress: true }, undefined, 'Deleting'],
  [true, {}, 'Status read failed', 'Status unavailable'],
  [true, { complete: true }, undefined, 'Deleted'],
] as const)(
  'presents acceptance=%s and observation=%j as %s / %s',
  (accepted, detail, readError, label) => {
    render(
      <DeletionStatusPanel
        request={{ ...request, accepted }}
        observation={{ ...observation, ...detail }}
        sending={false}
        readError={readError}
      />,
    );
    expect(screen.getByText(label, { exact: true })).toBeVisible();
    if (!('complete' in detail && detail.complete))
      expect(screen.queryByText(/Cleanup confirmed in this view/)).not.toBeInTheDocument();
    else expect(screen.getByText(/Cleanup confirmed in this view/)).toBeVisible();
  },
);

it('labels the read time without presenting an inferred cleanup step as a confirmed event', () => {
  render(
    <DeletionStatusPanel
      request={{ ...request, accepted: true }}
      observation={observation}
      sending={false}
    />,
  );
  expect(screen.getByText(/Last status read:/)).toBeVisible();
  expect(screen.queryByText(/Last confirmed: Stop Agents/)).not.toBeInTheDocument();
});

it.each(['denied', 'missing'] as const)(
  'keeps the full request reference selectable when clipboard is %s',
  async (failure) => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value:
        failure === 'missing'
          ? undefined
          : {
              writeText: vi
                .fn()
                .mockRejectedValue(new DOMException('Synthetic denial', 'NotAllowedError')),
            },
    });
    render(<DeletionStatusPanel request={request} observation={observation} sending={false} />);
    fireEvent.click(screen.getByRole('button', { name: 'Copy reference' }));
    expect(await screen.findByText(/Copy unavailable/)).toBeVisible();
    expect(screen.getByText(request.key)).toBeVisible();
  },
);

it('copies the original reference and does not issue a command', async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
  render(<DeletionStatusPanel request={request} observation={observation} sending={false} />);
  fireEvent.click(screen.getByRole('button', { name: 'Copy reference' }));
  expect(await screen.findByText('Reference copied.')).toBeVisible();
  expect(writeText).toHaveBeenCalledExactlyOnceWith(request.key);
});

it('stops all deletion observation after unmount without resubmitting', async () => {
  vi.useFakeTimers();
  const readSandbox = vi.fn(async () => sandboxFixture('retiring'));
  const deleteSandbox = vi.fn();
  const view = render(
    <DeletionDialog
      viewerId="owner"
      client={agentClient({ readSandbox, deleteSandbox })}
      initial={{ ...request, accepted: true }}
      restored
      onChanged={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await act(async () => {});
  expect(within(screen.getByRole('dialog')).getByText('Deleting', { exact: true })).toBeVisible();
  view.unmount();
  const count = readSandbox.mock.calls.length;
  await act(() => vi.advanceTimersByTimeAsync(60000));
  expect(readSandbox).toHaveBeenCalledTimes(count);
  expect(deleteSandbox).not.toHaveBeenCalled();
});
