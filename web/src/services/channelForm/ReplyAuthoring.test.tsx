import { RelayCapabilitiesContext } from '../RelayCapabilities';
import { available } from './NativeLarkSourceFields.fixtures';
import { validateReplyFields } from './replyValidation';
import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelPolicyFields } from './ChannelPolicyFields';
import { ChannelEditor } from '../ChannelEditor';
import { channel } from './NativeLarkSourceFields.fixtures';
import { defaultSource } from './model';

afterEach(() => vi.unstubAllGlobals());
const verified = { ...defaultSource('nyxid_relay'), platform: 'lark', bot_id: 'ou_bot' };
it.each([
  [defaultSource('nyxid_relay')],
  [defaultSource('telegram')],
  [{ ...verified, bot_id: undefined }],
  [{ ...verified, trust_normalized: true }],
  [verified, verified],
])('refuses new thread following on unsupported sources %j', (...sources) => {
  render(<ChannelPolicyFields value={{ sources }} onChange={vi.fn()} />);
  expect(screen.getByLabelText('Enable verified thread following')).toBeDisabled();
});
it.each(['lark', 'feishu'])('allows a single verified %s source', (platform) => {
  const onChange = vi.fn();
  render(
    <ChannelPolicyFields value={{ sources: [{ ...verified, platform }] }} onChange={onChange} />,
  );
  fireEvent.click(screen.getByLabelText('Enable verified thread following'));
  expect(onChange).toHaveBeenCalledWith(
    expect.objectContaining({ reply: { thread_contract: { version: 1, follow_chat_ids: [] } } }),
  );
});
it.each(['-1', '0.5', '60001', '999999999999999'])(
  'rejects acknowledgement delay %s before submission',
  (value) => {
    const fetcher = vi.fn();
    vi.stubGlobal('fetch', fetcher);
    render(<ChannelEditor channel={channel} onDone={vi.fn()} denied={vi.fn()} />);
    const input = screen.getByLabelText('Acknowledgement delay (ms)');
    fireEvent.change(input, { target: { value } });
    expect(input).toBeInvalid();
    fireEvent.submit(screen.getByRole('button', { name: 'Save channel' }).closest('form')!);
    expect(fetcher).not.toHaveBeenCalled();
    expect(input).toHaveFocus();
  },
);

it.each([0, 1, 60_000, 90_000])(
  'preserves a saved acknowledgement delay %s during an unrelated save',
  async (delay) => {
    const saved = {
      ...channel,
      definition: { ...channel.definition, reply: { ack_after_ms: delay } },
    };
    const fetcher = vi.fn().mockResolvedValue(new Response(JSON.stringify(saved)));
    vi.stubGlobal('fetch', fetcher);
    render(<ChannelEditor channel={saved} onDone={vi.fn()} denied={vi.fn()} />);
    const input = screen.getByLabelText('Acknowledgement delay (ms)');
    expect(input).toHaveValue(delay);
    expect(input).toBeValid();
    fireEvent.change(screen.getByLabelText('Channel name'), { target: { value: 'Renamed' } });
    fireEvent.submit(screen.getByRole('button', { name: 'Save channel' }).closest('form')!);
    await screen.findByText('Changes saved.');
    expect(JSON.parse(fetcher.mock.calls[0]![1].body).reply.ack_after_ms).toBe(delay);
  },
);

it('keeps the thread write gate and retained unsupported policy without silently deleting it', () => {
  const contract = { version: 1, follow_chat_ids: ['chat_retained'] };
  const value = { sources: [defaultSource('telegram')], reply: { thread_contract: contract } };
  const onChange = vi.fn();
  const discovery = {
    ...available,
    authoring: { ...available.authoring!, thread_contract_writes_enabled: false },
  };
  const view = render(
    <RelayCapabilitiesContext value={discovery}>
      <ChannelPolicyFields value={{ sources: [verified] }} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  expect(screen.getByLabelText('Enable verified thread following')).toBeDisabled();
  view.rerender(
    <RelayCapabilitiesContext value={discovery}>
      <ChannelPolicyFields value={value} onChange={onChange} />
    </RelayCapabilitiesContext>,
  );
  expect(screen.getByLabelText('Enable verified thread following')).toBeChecked();
  expect(screen.getByLabelText('Follow chat IDs 1')).toHaveValue('chat_retained');
  expect(onChange).not.toHaveBeenCalled();
  expect(() => validateReplyFields({ ...value, name: 'Renamed' }, value)).not.toThrow();
  expect(() => validateReplyFields(value)).toThrow('Thread following requires');
  fireEvent.click(screen.getByLabelText('Enable verified thread following'));
  expect(onChange).toHaveBeenCalledWith({ ...value, reply: {} });
});
