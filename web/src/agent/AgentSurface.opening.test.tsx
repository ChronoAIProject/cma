import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, agentClient } from '../test/agent';
import { CmaClientProvider } from '../api/context';
import { sandboxFixture, workspaceFixture } from '../test/hierarchy';
import { conversationFromTranscript, foldAgentEvent } from './conversation';
import { AgentSurface } from './AgentSurface';
import { controller } from './AgentSurface.fixture';
import '../test/connectedAccounts';

it.each(['active', 'delivery', 'previous'] as const)(
  'opens model discovery from %s evidence before initial Agent success without remounting the composer',
  async (evidence) => {
    const models = vi.fn(async () => [
      { id: 'corrective-model', label: 'Corrective model', available: true },
    ]);
    const client = agentClient({
      readAgentModels: models,
      readAgentSettings: async () => ({
        model: 'saved-model',
        reasoning_effort: 'high',
        permission_mode: 'autonomous',
      }),
      updateAgentSettings: vi.fn(),
    });
    const starting = {
      ...AGENT_DETAIL,
      status: 'starting' as const,
      readyAt: undefined,
      sandbox: { ...AGENT_DETAIL.sandbox, status: 'installing' as const },
    };
    const draw = (credentialDelivered: boolean) => {
      const detail =
        credentialDelivered && evidence === 'previous'
          ? { ...starting, status: 'failed' as const, readyAt: AGENT_DETAIL.readyAt }
          : starting;
      const sandboxEvents =
        credentialDelivered && evidence === 'delivery'
          ? [
              {
                sandboxId: starting.sandbox.id,
                seq: 1,
                appendedAt: '2026-11-03T00:00:00Z',
                kind: { type: 'work_step_completed', step: { kind: 'deliver_initial_credential' } },
              },
            ]
          : [];
      return (
        <CmaClientProvider client={client}>
          <AgentSurface
            agentId="agt_1"
            controller={controller(
              {
                status: 'ready',
                detail,
                workspace: workspaceFixture(),
                conversation: conversationFromTranscript(AGENT_TRANSCRIPT),
                sandboxEvents,
              },
              undefined,
              {
                progressObservation: {
                  stale: false,
                  sandbox: {
                    ...sandboxFixture(),
                    id: starting.sandbox.id,
                    status: credentialDelivered && evidence === 'active' ? 'active' : 'installing',
                  },
                },
              },
            )}
            detailsOpen={false}
            onDetailsOpenChange={vi.fn()}
            onNavigate={vi.fn()}
          />
        </CmaClientProvider>
      );
    };
    const view = render(draw(false));
    await screen.findByRole('button', { name: /^Agent settings:/ });
    expect(models).not.toHaveBeenCalled();
    const editor = screen.getByRole('textbox', { name: 'Message' });
    fireEvent.change(editor, { target: { value: 'Keep the next message' } });
    fireEvent.click(screen.getByRole('button', { name: /^Agent settings:/ }));
    expect(models).not.toHaveBeenCalled();
    view.rerender(draw(true));
    await waitFor(() => expect(models).toHaveBeenCalledTimes(1));
    expect(screen.getByRole('textbox', { name: 'Message' })).toBe(editor);
    expect(editor).toHaveValue('Keep the next message');
  },
);

it.each([false, true])(
  'links the current turn plan and preserves the draft with machine desktop=%s',
  (desktop) => {
    let conversation = conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] });
    const append = (seq: number, kind: Record<string, unknown>) => {
      conversation = foldAgentEvent(conversation, {
        seq,
        appendedAt: '2026-11-03T00:00:00Z',
        kind: kind as { type: string },
      });
    };
    append(1, { type: 'turn_started', turn_sequence: 1 });
    append(2, {
      type: 'plan_set',
      entries: [
        { title: 'Read instructions', status: 'completed' },
        { title: 'Apply change', status: 'in_progress' },
        { title: 'Verify', status: 'pending' },
      ],
    });
    const draw = () => (
      <AgentSurface
        agentId="agt_1"
        controller={controller({
          status: 'ready',
          detail: { ...AGENT_DETAIL, desktop },
          workspace: workspaceFixture(),
          conversation,
        })}
        detailsOpen={false}
        onDetailsOpenChange={() => {}}
        onNavigate={() => {}}
      />
    );
    const view = render(draw());
    expect(screen.getByRole('region', { name: 'Plan' }).closest('header')).toBeNull();
    expect(screen.queryByRole('link', { name: /^Current plan:/ })).toBeNull();
    fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
      target: { value: 'Retain next draft' },
    });
    append(3, { type: 'turn_completed', turn_sequence: 1 });
    append(4, { type: 'turn_started', turn_sequence: 4 });
    view.rerender(draw());
    expect(screen.queryByRole('link', { name: /^Current plan:/ })).not.toBeInTheDocument();
    expect(screen.getAllByRole('region', { name: 'Plan' })).toHaveLength(1);
    expect(
      screen.getByText('Response finished · Plan completion not fully reported'),
    ).toBeVisible();
    expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('Retain next draft');
  },
);

it('places settings inside the composer box and preserves draft and busy-turn apply rules', async () => {
  const defaults = {
    model: 'gpt-6',
    reasoning_effort: 'high',
    permission_mode: 'autonomous',
  } as const;
  const update = vi.fn().mockResolvedValue({ ...defaults, reasoning_effort: 'medium' });
  const client = agentClient({
    readAgentSettings: async () => defaults,
    readAgentModels: async () => [{ id: 'gpt-6', label: 'GPT-6', available: true }],
    updateAgentSettings: update,
  });
  const draw = (busy: boolean) => (
    <CmaClientProvider client={client}>
      <AgentSurface
        agentId="agt_1"
        controller={controller({
          status: 'ready',
          detail: { ...AGENT_DETAIL, status: busy ? 'working' : 'ready' },
          workspace: workspaceFixture(),
          conversation: conversationFromTranscript(AGENT_TRANSCRIPT),
        })}
        detailsOpen={false}
        onDetailsOpenChange={vi.fn()}
        onNavigate={vi.fn()}
      />
    </CmaClientProvider>
  );
  const view = render(draw(true));
  fireEvent.click(await screen.findByRole('button', { name: /^Agent settings:/ }));
  const chip = screen.getByRole('button', { name: /^Model and reasoning effort/ });
  await waitFor(() => expect(chip).toHaveTextContent('GPT-6 High'));
  const form = screen.getByRole('form', { name: 'Message Agent' });
  // Settings sit inside the composer box, in the row of controls under the draft.
  const controls = form.querySelector('.agent-composer__box > .agent-composer__controls');
  expect(controls).not.toBeNull();
  expect(controls!.previousElementSibling).toBe(screen.getByRole('textbox', { name: 'Message' }));
  expect(controls).toContainElement(chip);
  expect(controls).toContainElement(screen.getByRole('button', { name: 'Permission mode' }));
  expect(screen.queryByRole('button', { name: 'Apply settings' })).not.toBeInTheDocument();
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
    target: { value: 'Retain this draft' },
  });
  const actor = userEvent.setup();
  await actor.click(chip);
  const slider = screen.getByRole('slider', { name: 'Reasoning effort' });
  await actor.keyboard('{ArrowLeft}');
  expect(slider).toHaveAttribute('aria-valuetext', 'Medium');
  expect(screen.getByRole('button', { name: 'Apply settings' })).toBeDisabled();
  expect(controls).toContainElement(screen.getByRole('button', { name: 'Apply settings' }));
  expect(update).not.toHaveBeenCalled();
  view.rerender(draw(false));
  expect(chip).toHaveTextContent('GPT-6 Medium');
  await actor.click(screen.getByRole('button', { name: 'Apply settings' }));
  expect(update).toHaveBeenCalledWith(
    'agt_1',
    { ...defaults, reasoning_effort: 'medium' },
    expect.objectContaining({ idempotencyKey: expect.any(String) }),
  );
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('Retain this draft');
});
