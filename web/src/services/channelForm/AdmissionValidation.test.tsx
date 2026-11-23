import { useRef, useState } from 'react';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { AdvancedFields } from './AdvancedFields';
import { TextField } from './Fields';
import { SetupPolicy } from './SetupSections';
import { ValidationError } from './ValidationError';
import type { Fields } from './model';

const allowlist = { type: 'allowlist', sender_ids: ['sender-a', 'sender-a'] };
const scoped = {
  type: 'scoped',
  senders: { type: 'allowlist', ids: ['sender-b', 'sender-b'] },
  chats: { type: 'allowlist', ids: ['chat-b', 'chat-b'] },
  groups: 'deny',
};
const draft = {
  sources: [
    { type: 'nyxid_relay', admission: allowlist },
    { type: 'nyxid_relay', admission: scoped },
  ],
};

function Form({
  value,
  error,
  onChange,
}: {
  value: Fields;
  error: string;
  onChange: (value: Fields) => void;
}) {
  const root = useRef<HTMLDivElement>(null);
  const [refused, setRefused] = useState(false);
  return (
    <div ref={root}>
      <TextField label="Channel name" value="Retained draft" onChange={vi.fn()} />
      <AdvancedFields title="Unrelated connection settings">
        <TextField label="Route" value="retained-route" onChange={vi.fn()} />
      </AdvancedFields>
      <SetupPolicy value={value} onChange={onChange} />
      <button onClick={() => setRefused(true)}>Show refusal</button>
      {refused ? <ValidationError error={error} canEdit root={root} /> : null}
    </div>
  );
}

it.each([
  ['/sources/0/admission', 'Allowed sender IDs 1', 'sender-a'],
  ['/sources/0/admission/1', 'Allowed sender IDs 2', 'sender-a'],
  ['/sources/1/admission/senders', 'Senders IDs 1', 'sender-b'],
  ['/sources/1/admission/senders/1', 'Senders IDs 2', 'sender-b'],
  ['/sources/1/admission/chats', 'Chats IDs 1', 'chat-b'],
  ['/sources/1/admission/chats/1', 'Chats IDs 2', 'chat-b'],
])('focuses %s without opening unrelated settings or changing the draft', (path, label, value) => {
  const onChange = vi.fn();
  render(<Form value={draft} error={`${path}: values must be unique`} onChange={onChange} />);
  fireEvent.click(screen.getByRole('button', { name: 'Show refusal' }));
  const target = screen.getByLabelText(label);
  expect(target).toHaveFocus();
  expect(target).toHaveValue(value);
  expect(target).toHaveAccessibleDescription('Channel settings: values must be unique');
  fireEvent.click(screen.getByRole('button', { name: 'Review channel fields' }));
  expect(target).toHaveFocus();
  expect(screen.getByLabelText('Route').closest('details')).not.toHaveAttribute('open');
  expect(screen.getByLabelText('Allowed sender IDs 2')).toHaveValue('sender-a');
  expect(screen.getByLabelText('Senders IDs 2')).toHaveValue('sender-b');
  expect(screen.getByLabelText('Chats IDs 2')).toHaveValue('chat-b');
  expect(onChange).not.toHaveBeenCalled();
});

for (const prefix of ['/connection_admission', '/sources/0/admission']) {
  it.each([
    ['', allowlist, 'Allowed sender IDs 1'],
    ['/senders/1', scoped, 'Senders IDs 2'],
    ['/chats', scoped, 'Chats IDs 1'],
  ] as const)(
    'maps managed admission ' + prefix + '%s to its visible policy',
    (suffix, admission, label) => {
      const onChange = vi.fn();
      render(
        <Form
          value={{ connection_ref: 'owned-ready', connection_admission: admission, sources: [] }}
          error={`${prefix}${suffix}: invalid value`}
          onChange={onChange}
        />,
      );
      fireEvent.click(screen.getByRole('button', { name: 'Show refusal' }));
      const policy = within(screen.getByRole('group', { name: 'Connection admission' }));
      expect(policy.getByLabelText(label)).toHaveFocus();
      fireEvent.click(screen.getByRole('button', { name: 'Review channel fields' }));
      expect(policy.getByLabelText(label)).toHaveFocus();
      expect(screen.getByLabelText('Route').closest('details')).not.toHaveAttribute('open');
      expect(onChange).not.toHaveBeenCalled();
    },
  );
}

it('keeps unknown errors at the summary without opening unrelated settings', () => {
  render(<Form value={draft} error="/unmapped: invalid configuration" onChange={vi.fn()} />);
  fireEvent.click(screen.getByRole('button', { name: 'Show refusal' }));
  expect(screen.getByRole('alert')).toHaveFocus();
  fireEvent.click(screen.getByRole('button', { name: 'Review channel fields' }));
  expect(screen.getByRole('alert')).toHaveFocus();
  expect(screen.getByLabelText('Route').closest('details')).not.toHaveAttribute('open');
});
