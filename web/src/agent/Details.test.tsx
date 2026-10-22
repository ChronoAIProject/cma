import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import type { AgentFilesClient } from '../api/agentFiles';
import type { ResourceViewsClient } from '../resources/client';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { NamedReferences } from '../test/references';
import { AgentDetails } from './Details';

it('summarises service access read-only beside skills without duplicating composer settings', async () => {
  const readAgentSettings = vi.fn();
  const client = agentClient({ readAgentSettings });
  render(
    <CmaClientProvider client={client}>
      <NamedReferences>
        <AgentDetails detail={{ ...AGENT_DETAIL, reach: ['ornn-api', 'github'] }} open />
      </NamedReferences>
    </CmaClientProvider>,
  );
  const panel = within(screen.getByRole('complementary', { name: 'Agent details' }));
  expect(panel.queryByLabelText('Model')).not.toBeInTheDocument();
  expect(panel.queryByLabelText('Reasoning effort')).not.toBeInTheDocument();
  expect(panel.queryByLabelText('Permission mode')).not.toBeInTheDocument();
  expect(panel.queryByRole('group', { name: 'Agent key scope' })).not.toBeInTheDocument();
  expect(panel.queryByRole('checkbox')).not.toBeInTheDocument();
  expect(panel.queryByRole('button', { name: /Save scope/ })).not.toBeInTheDocument();
  await screen.findByRole('link', { name: 'GitHub' });
  const services = within(panel.getByRole('list', { name: 'Services this Agent can use' }));
  expect(services.getAllByRole('listitem').map((item) => item.textContent)).toEqual([
    'chrono-llm-publicRequired',
    'ornn-apiRequired',
    'GitHub',
  ]);
  expect(panel.getByRole('link', { name: 'Manage on Sandbox' })).toHaveAttribute(
    'href',
    '/sandboxes/sbx_1?tab=access',
  );
  expect(panel.getAllByRole('heading').map((heading) => heading.textContent)).toEqual([
    'Details',
    'Service access',
    'Skills',
  ]);
  expect(panel.queryByText(AGENT_DETAIL.workspace.path)).not.toBeInTheDocument();
  expect(panel.queryByText(AGENT_DETAIL.worktree.branch)).not.toBeInTheDocument();
  expect(panel.queryByRole('button', { name: 'Stop' })).not.toBeInTheDocument();
  expect(panel.queryByRole('button', { name: 'Delete' })).not.toBeInTheDocument();
  expect(readAgentSettings).not.toHaveBeenCalled();
});

describe('reaching the Sandbox page', () => {
  afterEach(() => {
    window.history.replaceState(null, '', '/');
  });
  const draw = (onOpenSandbox?: () => void) =>
    render(
      <CmaClientProvider client={agentClient()}>
        <AgentDetails detail={AGENT_DETAIL} open onOpenSandbox={onOpenSandbox} />
      </CmaClientProvider>,
    );
  const address = () => window.location.pathname + window.location.search;
  // Console's navigation pushes the bare Sandbox path, as the router does.
  const navigate = vi.fn(() => window.history.pushState(null, '', '/sandboxes/sbx_1'));

  it('opens the Access tab from Manage on Sandbox', async () => {
    draw(navigate);
    await userEvent.click(screen.getByRole('link', { name: 'Manage on Sandbox' }));
    expect(navigate).toHaveBeenCalledOnce();
    expect(address()).toBe('/sandboxes/sbx_1?tab=access');
  });

  it('opens the Activity tab from Sandbox operation progress', async () => {
    draw(navigate);
    await userEvent.click(screen.getByRole('button', { name: 'Sandbox operation progress' }));
    expect(address()).toBe('/sandboxes/sbx_1?tab=activity');
  });

  it('leaves a modified click, or a drawer without navigation, to the browser', () => {
    navigate.mockClear();
    const view = draw(navigate);
    const link = screen.getByRole('link', { name: 'Manage on Sandbox' });
    expect(fireEvent.click(link, { ctrlKey: true })).toBe(true);
    expect(navigate).not.toHaveBeenCalled();
    view.unmount();
    draw();
    expect(
      screen.queryByRole('button', { name: 'Sandbox operation progress' }),
    ).not.toBeInTheDocument();
    expect(fireEvent.click(screen.getByRole('link', { name: 'Manage on Sandbox' }))).toBe(true);
  });
});

it('focuses the Details heading on open and restores the opener on close', async () => {
  const onClose = vi.fn();
  const client = agentClient();
  const draw = (open: boolean) => (
    <CmaClientProvider client={client}>
      <button>Open details</button>
      <AgentDetails detail={AGENT_DETAIL} open={open} onClose={onClose} />
    </CmaClientProvider>
  );
  const view = render(draw(false));
  const opener = screen.getByRole('button', { name: 'Open details' });
  opener.focus();
  view.rerender(draw(true));
  expect(screen.getByRole('heading', { name: 'Details' })).toHaveFocus();
  await userEvent.click(screen.getByRole('button', { name: 'Close Agent details' }));
  expect(onClose).toHaveBeenCalledOnce();
  view.rerender(draw(false));
  expect(opener).toHaveFocus();
});

it('orders the calm sections and gives every text action the same quiet link', () => {
  // Neither client is read until its action is used, so inert stubs suffice.
  const files = {} as AgentFilesClient;
  const resourceViews = {} as ResourceViewsClient;
  const draw = (open: boolean) => (
    <CmaClientProvider client={agentClient({ files, resourceViews })}>
      <AgentDetails detail={AGENT_DETAIL} open={open} onOpenSandbox={vi.fn()} />
    </CmaClientProvider>
  );
  const view = render(draw(true));
  const panel = within(screen.getByRole('complementary', { name: 'Agent details' }));
  expect(panel.getAllByRole('heading').map((heading) => heading.textContent)).toEqual([
    'Details',
    'Files',
    'Sharing',
    'Service access',
    'Skills',
  ]);
  const actions = [
    panel.getByRole('button', { name: 'Browse files' }),
    panel.getByRole('button', { name: 'Manage share links' }),
    panel.getByRole('link', { name: 'Manage on Sandbox' }),
    panel.getByRole('button', { name: 'Sandbox operation progress' }),
  ];
  for (const action of actions) {
    expect(action).toHaveClass('agent-details__link');
    expect(action).not.toHaveClass('button');
  }
  expect(panel.getByRole('button', { name: 'Manage share links' })).toBeEnabled();
  view.rerender(draw(false));
  expect(screen.getByRole('button', { name: 'Manage share links' })).toBeDisabled();
});
