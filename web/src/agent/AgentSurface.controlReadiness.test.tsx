import { act, fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { AGENT_DETAIL } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import { conversationFromTranscript } from './conversation';
import { controller } from './AgentSurface.fixture';
import { AgentSurface } from './AgentSurface';

it.each(['loading', 'unavailable'] as const)(
  'enables observed turn controls and queue actions while history is %s',
  async (history) => {
    const row = {
      responseId: 'queued',
      commandId: 'queued-command',
      revision: 1,
      input: 'Waiting input',
    };
    const queuedInputs = {
      items: [row],
      outcomes: [],
      managedCommandIds: new Set(['queued-command']),
      edit: vi.fn(),
      remove: vi.fn(),
      recover: vi.fn(),
      refresh: vi.fn(),
    };
    const controls = controller(
      {
        status: 'ready',
        detail: { ...AGENT_DETAIL, status: 'working' },
        workspace: workspaceFixture(),
        conversation: {
          ...conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] }),
          activeRunId: '1',
          approvals: [{ id: 'live', reason: 'Write', message: 'Apply live change' }],
        },
        historyLoading: history === 'loading',
        historyUnavailable: history === 'unavailable',
      },
      undefined,
      { controlBaselineReady: true, queuedInputs },
    );
    const view = render(
      <AgentSurface
        agentId="agt_1"
        controller={controls}
        detailsOpen={false}
        onDetailsOpenChange={vi.fn()}
        onNavigate={vi.fn()}
      />,
    );
    expect(await screen.findByRole('button', { name: 'Allow' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Stop response' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Remove queued message 1' })).toBeEnabled();
    fireEvent.click(screen.getByRole('button', { name: 'Allow' }));
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Stop response' }));
    });
    fireEvent.click(screen.getByRole('button', { name: 'Remove queued message 1' }));
    expect(controls.approve).toHaveBeenCalledExactlyOnceWith('live', 'allow');
    expect(controls.interrupt).toHaveBeenCalledOnce();
    expect(queuedInputs.remove).toHaveBeenCalledExactlyOnceWith(row);
    view.rerender(
      <AgentSurface
        agentId="agt_1"
        controller={{ ...controls, controlBaselineReady: false }}
        detailsOpen={false}
        onDetailsOpenChange={vi.fn()}
        onNavigate={vi.fn()}
      />,
    );
    expect(screen.getByRole('button', { name: 'Allow' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Stop response' })).toBeDisabled();
  },
);
