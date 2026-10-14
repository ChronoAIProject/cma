import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
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
  expect(document.title).toBe('Repository introduction \u00b7 CMA');
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
