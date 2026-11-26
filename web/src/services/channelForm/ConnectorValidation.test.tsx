import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor } from '../ChannelEditor';
import { RelayCapabilitiesContext } from '../RelayCapabilities';
import {
  available,
  channel,
  enter,
  enterBundle,
  identity,
  response,
  source,
} from './NativeLarkSourceFields.fixtures';
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

it('returns focus to the Direct Lark webhook confirmation before sending a request', () => {
  const fetcher = vi.fn();
  vi.stubGlobal('fetch', fetcher);
  const saved = {
    ...channel,
    definition: {
      ...channel.definition,
      sources: [{ ...defaultSource('telegram'), bot_id: '123' }, source],
    },
  };
  render(
    <RelayCapabilitiesContext value={available}>
      <ChannelEditor
        channel={saved}
        section={{ kind: 'producer', index: 1 }}
        onDone={vi.fn()}
        denied={vi.fn()}
      />
    </RelayCapabilitiesContext>,
  );
  const confirmation = screen.getByRole('checkbox', { name: /This is a separate app/ });
  fireEvent.click(confirmation);
  expect(confirmation).not.toBeChecked();
  fireEvent.submit(screen.getByRole('button', { name: 'Save producer' }).closest('form')!);
  expect(fetcher).not.toHaveBeenCalled();
  expect(screen.getByRole('alert')).toHaveTextContent('Confirm that this is a separate app');
  const review = screen.getByRole('button', { name: 'Review channel fields' });
  review.focus();
  fireEvent.click(review);
  expect(confirmation).toHaveFocus();
  expect(confirmation).toHaveAccessibleDescription(/Confirm that this is a separate app/);
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

it('requires an allowed sender in a new direct Lark channel, including after removing the last sender', async () => {
  const fetcher = vi.fn().mockImplementation(async () => response());
  vi.stubGlobal('fetch', fetcher);
  render(
    <RelayCapabilitiesContext value={available}>
      <ChannelEditor ownerSubject="local-owner" onDone={vi.fn()} denied={vi.fn()} />
    </RelayCapabilitiesContext>,
  );
  enter('Channel name', 'Lark channel');
  enter('Provider agent key', 'fixture-agent-key');
  fireEvent.click(screen.getByRole('button', { name: 'Producer 1 connection type' }));
  fireEvent.click(screen.getByRole('menuitemradio', { name: 'Direct Lark / Feishu app' }));
  const senders = within(screen.getByRole('group', { name: 'Allowed sender IDs' }));
  const add = senders.getByRole('button', { name: 'Add Allowed sender IDs' });
  const missingSender = 'Add at least one allowed sender ID.';
  expect(senders.getByText(missingSender)).toBeVisible();
  expect(add).toHaveAccessibleDescription(missingSender);

  enter('App ID', identity.app_id);
  enter('Tenant key', identity.tenant_key);
  enter('Bot open ID', identity.bot_open_id);
  fireEvent.click(screen.getByRole('checkbox', { name: /This is a separate app/ }));
  enterBundle();

  const create = screen.getByRole('button', { name: 'Create channel' });
  fireEvent.click(create);
  expect(senders.getByText(missingSender)).toBeVisible();
  expect(add).toHaveFocus();
  expect(fetcher).not.toHaveBeenCalled();

  fireEvent.click(add);
  enter('Allowed sender IDs 1', '   ');
  expect(senders.getByText(missingSender)).toBeVisible();
  fireEvent.click(create);
  expect(senders.getByLabelText('Allowed sender IDs 1')).toHaveFocus();
  expect(senders.getByText('Enter a sender ID.')).toBeVisible();
  expect(fetcher).not.toHaveBeenCalled();

  enter('Allowed sender IDs 1', 'ou_fixture_sender');
  expect(senders.queryByText(missingSender)).not.toBeInTheDocument();
  fireEvent.click(senders.getByRole('button', { name: 'Remove Allowed sender IDs 1' }));
  expect(senders.getByText(missingSender)).toBeVisible();
  expect(add).toHaveAccessibleDescription(missingSender);
  fireEvent.click(create);
  expect(add).toHaveFocus();
  expect(fetcher).not.toHaveBeenCalled();

  fireEvent.click(add);
  enter('Allowed sender IDs 1', 'ou_fixture_sender');
  fireEvent.click(create);
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  expect(JSON.parse(fetcher.mock.calls[0]![1].body).sources[0].admission).toEqual({
    type: 'allowlist',
    sender_ids: ['ou_fixture_sender'],
  });
});
