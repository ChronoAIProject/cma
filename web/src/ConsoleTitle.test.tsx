import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
import type { AgentLiveEvent, AgentRuntimeMetadata } from './api/types';
import { Console } from './Console';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, agentClient } from './test/agent';
import { workspaceFixture } from './test/hierarchy';

afterEach(() => vi.useRealTimers());

it('adopts a title generated after the completed first turn without reloading or losing the draft', async () => {
  vi.useFakeTimers();
  let generated = false;
  const readAgent = vi.fn(async () => ({ ...AGENT_DETAIL, title: undefined }));
  const readAgentTranscript = vi.fn(async () => AGENT_TRANSCRIPT);
  const runAgent = vi.fn();
  const client = agentClient({
    readAgent,
    readAgentTranscript,
    runAgent,
    readWorkspace: async () => {
      const workspace = workspaceFixture();
      return {
        ...workspace,
        agents: workspace.agents.map((agent) => ({
          ...agent,
          title: generated ? 'Repository introduction' : undefined,
          updatedAt: generated ? '2026-09-06T00:06:00Z' : agent.updatedAt,
        })),
      };
    },
  });
  const { unmount } = render(
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
  expect(screen.getByRole('heading', { name: 'Untitled agent' })).toBeInTheDocument();
  const composer = screen.getByRole('textbox', { name: 'Message' });
  composer.focus();
  fireEvent.change(composer, { target: { value: 'Keep this unsent draft' } });
  const messages = [...document.querySelectorAll('article.message')];
  expect(messages).toHaveLength(2);
  const history = messages.map((message) => message.textContent);

  generated = true;
  await act(() => vi.advanceTimersByTimeAsync(15000));
  expect(screen.getByRole('heading', { name: 'Repository introduction' })).toBeInTheDocument();
  expect(document.title).toBe('ChronoAI Managed Agent (CMA)');
  expect(screen.getByRole('link', { name: /Repository introduction/ })).toBeInTheDocument();
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  expect(composer).toHaveValue('Keep this unsent draft');
  expect(composer).toHaveFocus();
  expect([...document.querySelectorAll('article.message')]).toEqual(messages);
  expect(messages.map((message) => message.textContent)).toEqual(history);
  await act(() => vi.advanceTimersByTimeAsync(10000));
  expect(readAgent).toHaveBeenCalledTimes(1);
  expect(readAgentTranscript).toHaveBeenCalledTimes(1);
  expect(runAgent).not.toHaveBeenCalled();
  unmount();
});
import './test/connectedAccounts';

it('shares setup filtering and live native titles across header, document and sidebar without losing history or draft', async () => {
  vi.useFakeTimers();
  const setup = 'Prepare the empty workspace and wait for the owner.';
  const configuredTitle = 'Configured agent';
  const metadata: AgentRuntimeMetadata = {
    description: setup,
    descriptionSource: 'prompt_preview',
    descriptionIsSetupPreview: true,
  };
  let update: ((event: AgentLiveEvent) => void) | undefined;
  const readAgent = vi.fn(async () => ({
    ...AGENT_DETAIL,
    title: configuredTitle,
    runtimeMetadata: metadata,
  }));
  const readAgentTranscript = vi.fn(async () => AGENT_TRANSCRIPT);
  const runAgent = vi.fn();
  const client = agentClient({
    readAgent,
    readAgentTranscript,
    runAgent,
    readWorkspace: async () => {
      const workspace = workspaceFixture();
      return {
        ...workspace,
        agents: workspace.agents.map((agent) => ({
          ...agent,
          title: configuredTitle,
          runtimeMetadata: metadata,
        })),
      };
    },
    replayAgent: async (_id, options) => {
      update = options.onEvent;
    },
  });
  const { unmount } = render(
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
  const expectTitle = (title: string) => {
    expect(screen.getByRole('heading', { name: title })).toBeInTheDocument();
    expect(document.title).toBe('ChronoAI Managed Agent (CMA)');
    expect(document.querySelector('.tree-row[data-node-id="agent:agt_1"]')).toHaveTextContent(
      title,
    );
  };
  expectTitle(configuredTitle);
  const composer = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(composer, { target: { value: 'Keep this unsent draft' } });
  const messages = [...document.querySelectorAll('article.message')];
  const history = messages.map((message) => message.textContent);
  const publish = async (value: Record<string, unknown>, threadId = 'agt_1') => {
    await act(async () =>
      update!({
        type: 'STATE_DELTA',
        threadId,
        runId: '1',
        delta: [{ op: 'replace', path: '/runtimeMetadata', value }],
      }),
    );
  };
  await publish(
    { description: 'Unrelated Agent name', descriptionSource: 'provider_title' },
    'agt_other',
  );
  expectTitle(configuredTitle);
  await publish({ description: setup, descriptionSource: 'provider_title' });
  expectTitle(setup);
  await publish({ description: 'Native investigation title', descriptionSource: 'provider_title' });
  expectTitle('Native investigation title');
  await publish({
    description: 'Retained setup text after resume',
    descriptionSource: 'prompt_preview',
    descriptionIsSetupPreview: true,
  });
  expectTitle('Native investigation title');
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(composer);
  expect(composer).toHaveValue('Keep this unsent draft');
  expect([...document.querySelectorAll('article.message')]).toEqual(messages);
  expect(messages.map((message) => message.textContent)).toEqual(history);
  expect(readAgent).toHaveBeenCalledTimes(1);
  expect(readAgentTranscript).toHaveBeenCalledTimes(1);
  expect(runAgent).not.toHaveBeenCalled();
  unmount();
});
