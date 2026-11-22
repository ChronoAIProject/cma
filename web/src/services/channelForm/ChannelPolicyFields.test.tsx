import { render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { RelayCapabilitiesContext } from '../RelayCapabilities';
import { ChannelEditor, type Channel } from '../ChannelEditor';
import { ChannelPolicyFields } from './ChannelPolicyFields';
import { defaultSource, type Fields } from './model';

const guidance =
  "This channel's saved CMA-managed connection. When empty, the channel uses its producer settings. The reference cannot be added, removed, or replaced after channel creation. To use a different managed connection, create a new channel.";

const sources: [string, Fields[]][] = [
  ['direct Discord', [defaultSource('discord')]],
  ['Discord relay', [{ ...defaultSource('nyxid_relay'), platform: 'discord' }]],
  ['Telegram', [defaultSource('telegram')]],
  ['native Lark', [defaultSource('native_lark')]],
  [
    'native Feishu',
    [{ ...defaultSource('native_lark'), identity: { region: 'feishu', app_id: 'synthetic-app' } }],
  ],
  ['custom', [defaultSource('custom')]],
  ['mixed', [defaultSource('discord'), defaultSource('telegram')]],
  ['unknown', [{ type: 'future_source' }]],
];

it.each(sources)('shows neutral saved connection guidance for %s and on reopen', (_, producers) => {
  const channel: Channel = {
    channel_id: 'synthetic-channel',
    record_id: 'synthetic-record',
    version: 2,
    provider: { slug: 'cma_codex', level: 'l3' },
    definition: { name: 'Saved channel', sources: producers },
    endpoints: { custom_event_url: '', nyxid_callback_url: '' },
  };
  const editor = <ChannelEditor channel={channel} onDone={vi.fn()} denied={vi.fn()} />;
  const check = () => {
    const reference = screen.getByRole('textbox', { name: 'Connection reference' });
    expect(reference).toHaveValue('No shared connection');
    expect(reference).toHaveAttribute('readonly');
    expect(reference).toHaveAccessibleDescription(guidance);
    expect(screen.queryByText(/CMA-managed Telegram connection reference/)).not.toBeInTheDocument();
  };
  const view = render(editor);
  check();
  view.unmount();
  render(editor);
  check();
});

it('updates the displayed reference and neutral guidance when the channel changes', () => {
  const onChange = vi.fn();
  const { rerender } = render(
    <ChannelPolicyFields
      value={{ sources: [defaultSource('telegram')], connection_ref: 'Exact_First.Reference' }}
      onChange={onChange}
    />,
  );
  expect(screen.getByLabelText('Connection reference')).toHaveValue('Exact_First.Reference');
  for (const [, producers] of sources) {
    rerender(<ChannelPolicyFields value={{ sources: producers }} onChange={onChange} />);
    expect(screen.getByLabelText('Connection reference')).toHaveValue('No shared connection');
    expect(screen.getByLabelText('Connection reference')).toHaveAccessibleDescription(guidance);
  }
  rerender(
    <ChannelPolicyFields
      value={{ sources: [defaultSource('telegram')], connection_ref: 'Exact_Second.Reference' }}
      onChange={onChange}
    />,
  );
  expect(screen.getByLabelText('Connection reference')).toHaveValue('Exact_Second.Reference');
  expect(onChange).not.toHaveBeenCalled();
});

it('keeps the existing Telegram connection selector in new-channel setup', () => {
  render(
    <RelayCapabilitiesContext value={{ status: 'ready', managedTelegramSetup: true }}>
      <ChannelEditor onDone={vi.fn()} denied={vi.fn()} />
    </RelayCapabilitiesContext>,
  );
  expect(screen.queryByLabelText('Connection reference')).not.toBeInTheDocument();
  expect(screen.getByLabelText('Use an existing CMA-managed Telegram connection')).toBeVisible();
  expect(
    screen.getByLabelText('Use an existing CMA-managed Telegram connection'),
  ).not.toBeChecked();
});
