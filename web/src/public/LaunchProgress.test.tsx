import { render, screen, within } from '@testing-library/react';
import type { Launch, LaunchStep, LaunchStepStatus } from '@chronoai/cma-sdk';
import { describe, expect, it } from 'vitest';
import { PUBLIC_RECORD } from '../test/public';
import { LaunchProgress } from './LaunchProgress';

const setupSteps: LaunchStep[] = (
  ['signing_in', 'checking_requirements', 'sandbox', 'workspace', 'agent'] as const
).map((key) => ({ key, status: 'completed', safe_detail: '' }));
const prepared: Launch = { ...PUBLIC_RECORD, state: 'message_pending', steps: setupSteps };
const rows = () =>
  within(screen.getByRole('list', { name: 'Agent provisioning progress' })).getAllByRole(
    'listitem',
  );

describe('three provisioning phases', () => {
  it('waits for durable Agent startup before completing runtime setup and key access', () => {
    const starting: Launch = {
      ...prepared,
      state: 'agent_pending',
      steps: setupSteps.map((step) =>
        step.key === 'agent' ? { ...step, status: 'running' } : step,
      ),
    };
    const { rerender } = render(<LaunchProgress record={starting} />);
    expect(screen.queryByRole('region', { name: 'Signing you in' })).not.toBeInTheDocument();
    expect(
      screen.queryByRole('region', { name: 'NyxID Service Connection Verification' }),
    ).not.toBeInTheDocument();
    expect(rows()).toHaveLength(3);
    expect(rows().map((row) => within(row).getByRole('heading').textContent)).toEqual([
      'Sandbox ready',
      'Workspace + skills + model + reasoning effort + system prompt + permission level setup ready',
      'Agent key scope configuration ready',
    ]);
    expect(rows()[0]).toHaveTextContent('Done');
    expect(rows()[1]).toHaveTextContent('In progress');
    expect(rows()[2]).toHaveTextContent('Waiting');
    rerender(<LaunchProgress record={prepared} />);
    rows().forEach((row) => expect(row).toHaveTextContent('Done'));
  });

  it.each(['sandbox', 'workspace', 'agent', 'checking_requirements'] as const)(
    'does not invent key readiness for a sparse historical record missing %s evidence',
    (missing) => {
      render(
        <LaunchProgress
          record={{
            ...prepared,
            state: 'ready',
            steps: setupSteps.filter((step) => step.key !== missing),
          }}
        />,
      );
      expect(rows()[2]).toHaveTextContent('Waiting');
      expect(rows()[2]).not.toHaveTextContent('Done');
    },
  );

  it.each(['created', 'reused'] as const)(
    'shows completed setup for %s resources when no optional message is needed',
    (state) => {
      render(
        <LaunchProgress
          record={{
            ...prepared,
            state: 'ready',
            resolution: {
              sandbox: { state, id: 'sb_ready' },
              workspace: { state, id: 'ws_ready' },
              agent: { state, id: 'agt_ready' },
              choice_required: false,
            },
            first_message: 'completed',
            steps: [
              ...setupSteps,
              {
                key: 'first_message',
                status: 'completed',
                safe_detail:
                  state === 'reused' ? 'Existing conversation preserved' : 'No first message',
              },
            ],
          }}
        />,
      );
      rows().forEach((row) => expect(row).toHaveTextContent('Done'));
      expect(screen.getByRole('region', { name: 'Sending your first message' })).toHaveTextContent(
        state === 'reused' ? 'Existing conversation preserved' : 'No first message',
      );
    },
  );

  it('does not treat connected services and a skipped message as prepared resources', () => {
    render(
      <LaunchProgress
        record={{
          ...PUBLIC_RECORD,
          first_message: 'completed',
          steps: [
            ...setupSteps.filter((step) =>
              ['signing_in', 'checking_requirements'].includes(step.key),
            ),
            { key: 'first_message', status: 'completed', safe_detail: 'No first message' },
          ],
        }}
      />,
    );
    rows().forEach((row) => expect(row).toHaveTextContent('Waiting'));
  });

  it.each(['signing_in', 'checking_requirements'] as const)(
    'keeps a failed %s prerequisite separate from unallocated resources',
    (key) => {
      render(
        <LaunchProgress
          record={{
            ...PUBLIC_RECORD,
            state: 'failed_terminal',
            steps: [
              { key, status: 'failed', safe_detail: 'Please sign in and check your accounts.' },
            ],
          }}
        />,
      );
      rows().forEach((row) => expect(row).toHaveTextContent('Waiting'));
      expect(screen.getByRole('region')).toHaveTextContent("Didn't work");
      expect(screen.getByRole('region')).toHaveTextContent(
        'Please sign in and check your accounts.',
      );
    },
  );

  it.each([
    ['queued', 'running', 'In progress', 'Waiting for the first message'],
    ['dispatch_fenced', 'retryable', 'Waiting', 'First-message delivery will be retried'],
    ['recovery_required', 'blocked', 'Needs you', 'First-message delivery needs review'],
    ['failed', 'failed', "Didn't work", 'The first message could not be delivered'],
  ] satisfies [Launch['first_message'], LaunchStepStatus, string, string][])(
    'keeps %s first-message status outside completed provisioning phases',
    (firstMessage, status, label, detail) => {
      render(
        <LaunchProgress
          record={{
            ...prepared,
            first_message: firstMessage,
            blocked_reason:
              firstMessage === 'recovery_required' ? { type: 'recovery_required' } : undefined,
            steps: [...setupSteps, { key: 'first_message', status, safe_detail: detail }],
          }}
        />,
      );
      rows().forEach((row) => {
        expect(row).toHaveTextContent('Done');
        expect(row).not.toHaveTextContent("Didn't work");
      });
      const message = screen.getByRole('region', { name: 'Sending your first message' });
      expect(message).toHaveTextContent(label);
      expect(message).toHaveTextContent(detail);
      expect(message.closest('ol')).toBeNull();
    },
  );
});
