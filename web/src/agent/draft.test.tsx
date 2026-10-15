import { act, fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { Composer } from './Composer';
import { useAgentDraft } from './draft';

it.each([
  ['Original message', ''],
  ['Edited draft', 'Edited draft'],
])('retains only unaccepted input across composer remounts: %s', (content, expected) => {
  const client = agentClient();
  function Surface({ visible, accepted }: { visible: boolean; accepted?: string }) {
    const draft = useAgentDraft(client, 'owner', 'agt_1');
    return visible ? (
      <Composer
        draft={draft}
        status="ready"
        running={false}
        busy={false}
        acceptedMessage={accepted}
        inputAvailability={{ allowed: true }}
        onSend={vi.fn()}
        onStop={vi.fn()}
      />
    ) : null;
  }
  const view = render(<Surface visible />);
  const field = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(field, { target: { value: 'Original message' } });
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  if (content !== 'Original message') fireEvent.change(field, { target: { value: content } });
  view.rerender(<Surface visible accepted="Original message" />);
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue(expected);
  view.rerender(<Surface visible={false} accepted="Original message" />);
  view.rerender(<Surface visible accepted="Original message" />);
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue(expected);
});

it('ignores late acceptance from a previous visit to the same Agent', () => {
  const client = agentClient();
  let accept: (() => void) | undefined;
  function Surface({ agentId }: { agentId: string }) {
    const draft = useAgentDraft(client, 'owner', agentId);
    return (
      <Composer
        key={agentId}
        draft={draft}
        status="ready"
        running={false}
        busy={false}
        inputAvailability={{ allowed: true }}
        onSend={(_content, onAccepted) => {
          accept = onAccepted;
        }}
        onStop={vi.fn()}
      />
    );
  }
  const view = render(<Surface agentId="agt_1" />);
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
    target: { value: 'Same text, new intent' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  expect(accept).toBeDefined();
  view.rerender(<Surface agentId="agt_2" />);
  view.rerender(<Surface agentId="agt_1" />);
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), {
    target: { value: 'Same text, new intent' },
  });
  act(() => accept?.());
  expect(screen.getByRole('textbox', { name: 'Message' })).toHaveValue('Same text, new intent');
});
