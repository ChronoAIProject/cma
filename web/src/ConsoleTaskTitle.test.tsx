import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
import type { AgentLiveEvent } from './api/types';
import { Console } from './Console';
import { AGENT_DETAIL, agentClient } from './test/agent';
import { workspaceFixture } from './test/hierarchy';
import './test/connectedAccounts';

afterEach(() => vi.useRealTimers());

it.each([
  {
    metadata: {
      description: 'Automatic preparation',
      descriptionSource: 'prompt_preview' as const,
      descriptionIsSetupPreview: true,
    },
    expected: 'Untitled task',
  },
  {
    metadata: { description: '   ', descriptionSource: 'provider_title' as const },
    expected: 'Untitled task',
  },
  {
    metadata: {
      description: 'Review release notes ' + 'for deployment '.repeat(10),
      descriptionSource: 'provider_title' as const,
    },
    expected: 'Review release notes for deployment for deployment for deploymen',
  },
  { metadata: { description: 'Legacy task description' }, expected: 'Legacy task description' },
])(
  'shows a consistent safe task name after public reads: $expected',
  async ({ metadata, expected }) => {
    vi.useFakeTimers();
    const client = agentClient({
      readAgent: async () => ({ ...AGENT_DETAIL, title: 'Agent agt_1', runtimeMetadata: metadata }),
      readWorkspace: async () => {
        const workspace = workspaceFixture();
        return {
          ...workspace,
          agents: workspace.agents.map((agent) => ({
            ...agent,
            title: 'Agent agt_1',
            runtimeMetadata: metadata,
          })),
        };
      },
    });
    const view = render(
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
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByRole('heading', { name: expected })).toBeVisible();
    expect(screen.getByRole('link', { name: new RegExp(expected) })).toBeVisible();
    expect(document.title).toBe(`${expected} · CMA`);
    view.unmount();
  },
);

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
      system_prompt: '',
      revision: 1,
      prompt_pending: false,
      prompt_supported: false,
    }),
    updateAgentPreferences: async (_id, request) => {
      ownerTitle = request.title;
      return {
        title: ownerTitle,
        system_prompt: '',
        revision: 2,
        prompt_pending: false,
        prompt_supported: false,
      };
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

it('keeps the first task preview through resumed setup, blank and unknown metadata until a valid provider title arrives', async () => {
  vi.useFakeTimers();
  let update: ((event: AgentLiveEvent) => void) | undefined;
  const client = agentClient({
    readAgent: async () => ({
      ...AGENT_DETAIL,
      runtimeMetadata: {
        description: 'Investigate checkout failures',
        descriptionSource: 'prompt_preview',
        descriptionIsSetupPreview: false,
      },
    }),
    replayAgent: async (_id, options) => {
      update = options.onEvent;
    },
  });
  const view = render(
    <CmaClientProvider client={client}>
      <Console
        user={{ id: 'owner', handle: 'Owner', visitor: false, capabilities: ['use_own_sandboxes'] }}
        route={{ kind: 'agent', agentId: 'agt_1' }}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>,
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  const composer = screen.getByRole('textbox', { name: 'Message' });
  composer.focus();
  fireEvent.change(composer, { target: { value: 'Keep this draft' } });
  for (const metadata of [
    {
      description: 'Prepare workspace',
      descriptionSource: 'prompt_preview',
      descriptionIsSetupPreview: true,
    },
    { description: '   ', descriptionSource: 'provider_title' },
    { description: 'Unknown update', descriptionSource: 'future_source' },
  ]) {
    await act(async () =>
      update!({
        type: 'STATE_DELTA',
        threadId: 'agt_1',
        runId: '1',
        delta: [{ op: 'replace', path: '/runtimeMetadata', value: metadata }],
      }),
    );
    expect(screen.getByRole('heading', { name: 'Investigate checkout failures' })).toBeVisible();
    expect(document.title).toBe('Investigate checkout failures · CMA');
  }
  await act(async () =>
    update!({
      type: 'STATE_DELTA',
      threadId: 'agt_1',
      runId: '1',
      delta: [
        {
          op: 'replace',
          path: '/runtimeMetadata',
          value: { description: 'Checkout failure analysis', descriptionSource: 'provider_title' },
        },
      ],
    }),
  );
  expect(screen.getByRole('heading', { name: 'Checkout failure analysis' })).toBeVisible();
  expect(screen.getByRole('link', { name: /Checkout failure analysis/ })).toBeVisible();
  expect(document.title).toBe('Checkout failure analysis · CMA');
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  expect(composer).toHaveValue('Keep this draft');
  expect(composer).toHaveFocus();
  view.unmount();
});
