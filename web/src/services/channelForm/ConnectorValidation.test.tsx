import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor } from '../ChannelEditor';
import { channel } from './NativeLarkSourceFields.fixtures';
import { SourceFields } from './SourceFields';
import { defaultSource } from './model';

afterEach(() => vi.unstubAllGlobals());
it.each([
  ['telegram', 'Bot ID', 'abc'],
  ['telegram', 'Bot ID', '0'],
  ['telegram', 'Bot ID', '01'],
  ['telegram', 'Bot ID', '9223372036854775808'],
  ['telegram', 'Bot username', '@invalid'],
  ['telegram', 'Bot username', 'abcd'],
  ['discord', 'Discord bot user ID', 'abc'],
  ['discord', 'Discord bot user ID', '18446744073709551616'],
  ['native_lark', 'App ID', 'no-prefix'],
  ['native_lark', 'Tenant key', 'spaces forbidden'],
  ['native_lark', 'Bot open ID', 'no-prefix'],
])('validates %s %s before transport and focuses only that field', (type, label, invalid) => {
  const fetcher = vi.fn();
  vi.stubGlobal('fetch', fetcher);
  const saved = {
    ...channel,
    definition: { ...channel.definition, sources: [defaultSource(type)] },
  };
  render(
    <ChannelEditor
      channel={saved}
      section={{ kind: 'producer', index: 0 }}
      onDone={vi.fn()}
      denied={vi.fn()}
    />,
  );
  // Fill other direct identities so the submitted error identifies the changed field.
  for (const [name, value] of [
    ['Bot ID', '123'],
    ['App ID', 'cli_app'],
    ['Tenant key', 'tenant'],
    ['Bot open ID', 'ou_bot'],
  ]) {
    const field = screen.queryByLabelText(name!);
    if (field) fireEvent.change(field, { target: { value } });
  }
  const target = screen.getByLabelText(label);
  fireEvent.change(target, { target: { value: invalid } });
  expect(target).toBeInvalid();
  expect(target).toHaveAttribute('aria-invalid', 'true');
  fireEvent.submit(screen.getByRole('button', { name: 'Save producer' }).closest('form')!);
  expect(fetcher).not.toHaveBeenCalled();
  expect(target).toHaveFocus();
  fireEvent.click(screen.getByRole('button', { name: 'Review channel fields' }));
  expect(target).toHaveFocus();
  expect(screen.getByRole('alert').textContent).not.toContain('/sources/');
});
it.each([
  ['telegram', 'Bot ID', { bot_id: '9223372036854775807' }],
  ['telegram', 'Bot username', { bot_username: 'valid_bot' }],
  ['discord', 'Discord bot user ID', { bot_id: '18446744073709551615' }],
  ['native_lark', 'App ID', { identity: { app_id: 'cli_' } }],
])('accepts the backend boundary for %s %s', (type, label, fields) => {
  render(<SourceFields value={{ ...defaultSource(type), ...fields }} onChange={vi.fn()} />);
  expect(screen.getByLabelText(label)).toBeValid();
});
