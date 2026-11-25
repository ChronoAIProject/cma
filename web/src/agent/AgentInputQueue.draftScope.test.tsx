import { render } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { AgentInputQueue } from './AgentInputQueue';
import type { AgentController } from './controller';
import { agentTransportJournal, clearAgentTransport } from './transport/journal';

const presentation = vi.hoisted(() => ({
  current: undefined as undefined | { canUse: () => boolean; onDismiss: (key: string) => void },
}));
vi.mock('./QueuedMessages', () => ({
  QueuedMessages: () => null,
  UnappliedQueuedEdits: (props: NonNullable<typeof presentation.current>) => {
    presentation.current = props;
    return null;
  },
}));
vi.mock('./QueuedInputOutcomes', () => ({ QueuedInputOutcomes: () => null }));

beforeEach(() => {
  sessionStorage.clear();
  presentation.current = undefined;
});

it('invalidates old local draft actions on access loss, Agent navigation, and a new epoch', () => {
  const first = agentTransportJournal(sessionStorage, 'owner', 'agt_1');
  first.select(true);
  const other = agentTransportJournal(sessionStorage, 'owner', 'agt_2');
  other.select(true);
  const dismiss = vi.fn();
  let identityMatches = true;
  const controller = {
    inputAvailability: { allowed: true },
    queuedInputs: {
      unappliedEdits: [{ key: 'key', text: 'draft' }],
      dismissUnappliedEdit: dismiss,
    },
  } as unknown as AgentController;
  const props = {
    controller,
    queue: { items: [], outcomes: [], messages: [] },
    readOnly: false,
    controlReady: true,
    verified: false,
    draftIdentityCurrent: () => identityMatches,
    onRestore: vi.fn(),
  };
  const view = render(<AgentInputQueue {...props} draftJournal={first} />);
  const old = presentation.current!;
  expect(old.canUse()).toBe(true);
  identityMatches = false;
  expect(old.canUse()).toBe(false);
  old.onDismiss('key');
  view.rerender(<AgentInputQueue {...props} />);
  expect(old.canUse()).toBe(false);
  old.onDismiss('key');
  identityMatches = true;
  view.rerender(<AgentInputQueue {...props} draftJournal={first} />);
  expect(old.canUse()).toBe(false);
  old.onDismiss('key');
  const reopened = presentation.current!;
  expect(reopened.canUse()).toBe(true);
  view.rerender(<AgentInputQueue {...props} draftJournal={other} />);
  expect(reopened.canUse()).toBe(false);
  reopened.onDismiss('key');
  const otherAction = presentation.current!;
  clearAgentTransport(sessionStorage);
  agentTransportJournal(sessionStorage, 'owner', 'agt_2').select(true);
  expect(otherAction.canUse()).toBe(false);
  otherAction.onDismiss('key');
  expect(dismiss).not.toHaveBeenCalled();
});
