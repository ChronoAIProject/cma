import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
import type { AgentLiveEvent } from './api/types';
import { Console } from './Console';
import { AGENT_DETAIL, agentClient } from './test/agent';
import { workspaceFixture } from './test/hierarchy';
import './test/connectedAccounts';

afterEach(() => vi.useRealTimers());

it('keeps the owner rename after a late provider title in navigation, header, browser and reopened task', async () => {
  vi.useFakeTimers();
  let ownerTitle: string | undefined;
  let providerTitle = 'Compare development tools';
  let update: ((event: AgentLiveEvent) => void) | undefined;
  const client = agentClient({
    readAgent: async () => ({
      ...AGENT_DETAIL,
      ownerTitle,
      runtimeMetadata: { description: providerTitle, descriptionSource: 'provider_title' },
    }),
    readWorkspace: async () => {
      const workspace = workspaceFixture();
      return {
        ...workspace,
        agents: workspace.agents.map((agent) => ({
          ...agent,
          ownerTitle,
          runtimeMetadata: { description: providerTitle, descriptionSource: 'provider_title' },
        })),
      };
    },
    readAgentPreferences: async () => ({
      title: ownerTitle,
      revision: 1,
      prompt_pending: false,
      prompt_supported: false,
    }),
    updateAgentPreferences: async (_id, request) => {
      ownerTitle = request.title;
      return { title: ownerTitle, revision: 2, prompt_pending: false, prompt_supported: false };
    },
    replayAgent: async (_id, options) => {
      update = options.onEvent;
    },
  });
  const draw = () =>
    render(
      <CmaClientProvider client={client}>
        <Console
          user={{
            id: 'owner',
            handle: 'Owner',
            visitor: false,
            capabilities: ['use_own_sandboxes'],
          }}
          route={{ kind: 'agent', agentId: 'agt_1' }}
          onNavigate={vi.fn()}
          onSignOut={vi.fn()}
        />
      </CmaClientProvider>,
    );
  let view = draw();
  await act(() => vi.advanceTimersByTimeAsync(0));
  const composer = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(composer, { target: { value: 'Keep my draft' } });
  const messages = [...document.querySelectorAll('article.message')];
  fireEvent.click(screen.getByRole('button', { name: 'More Agent actions' }));
  fireEvent.click(screen.getByRole('menuitem', { name: 'Rename Agent' }));
  await act(() => vi.advanceTimersByTimeAsync(0));
  fireEvent.change(screen.getByRole('textbox', { name: 'Agent name' }), {
    target: { value: '工具选型' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await act(() => vi.advanceTimersByTimeAsync(0));
  providerTitle = 'Old provider title';
  await act(async () =>
    update!({
      type: 'STATE_DELTA',
      threadId: 'agt_1',
      runId: '1',
      delta: [
        {
          op: 'replace',
          path: '/runtimeMetadata',
          value: {
            description: providerTitle,
            descriptionSource: 'provider_title',
          },
        },
      ],
    }),
  );
  const expectOwnerTitle = () => {
    expect(screen.getByRole('heading', { name: '工具选型' })).toBeVisible();
    expect(screen.getByRole('link', { name: /工具选型/ })).toBeVisible();
    expect(document.title).toBe('工具选型 · CMA');
  };
  expectOwnerTitle();
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  expect(composer).toHaveValue('Keep my draft');
  expect([...document.querySelectorAll('article.message')]).toEqual(messages);
  view.unmount();
  view = draw();
  await act(() => vi.advanceTimersByTimeAsync(0));
  expectOwnerTitle();
  view.unmount();
});
