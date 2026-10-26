import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { SourceFields } from './SourceFields';
import type { Fields } from './model';

function source(platform: string, verification: Fields): Fields {
  return {
    type: 'nyxid_relay',
    platform,
    issuer: 'https://nyxid.example.test',
    key_id: 'signing-key-fixture',
    route_ids: ['retained-route'],
    bot_id: '123',
    bot_username: 'retained_bot',
    admission: { type: 'allowlist', sender_ids: ['retained-sender'] },
    future_setting: { retained: true },
    ...verification,
  };
}

function selectPlatform(value: Fields, platform: string): Fields {
  const onChange = vi.fn();
  render(<SourceFields value={value} onChange={onChange} />);
  fireEvent.click(screen.getByRole('button', { name: 'Messaging platform' }));
  fireEvent.click(screen.getByRole('menuitemradio', { name: platform }));
  expect(onChange).toHaveBeenCalledTimes(1);
  return onChange.mock.calls[0]![0] as Fields;
}

it.each(
  ['discord', 'whatsapp', 'slack'].flatMap((from) =>
    ['telegram', 'lark', 'feishu'].map((to) => ({ from, to })),
  ),
)('removes unsupported trust settings when switching $from to $to', ({ from, to }) => {
  const original = source(from, { trust_normalized: true });
  const next = selectPlatform(original, to);
  expect(next).toEqual(source(to, {}));
  expect(next).not.toHaveProperty('trust_normalized');
  expect(next).not.toHaveProperty('verify_raw_event');
  expect(original.trust_normalized).toBe(true);
});

it.each(['telegram', 'lark', 'feishu', 'slack'])(
  'removes raw verification when switching to %s without enabling normalized trust',
  (platform) => {
    const original = source('discord', { verify_raw_event: true });
    const next = selectPlatform(original, platform);
    expect(next).toEqual(source(platform, {}));
    expect(next).not.toHaveProperty('trust_normalized');
  },
);

it.each([
  { from: 'discord', to: 'whatsapp', verification: { trust_normalized: true } },
  { from: 'whatsapp', to: 'discord', verification: { verify_raw_event: true } },
  { from: 'slack', to: 'discord', verification: { trust_normalized: true } },
  { from: 'discord', to: 'slack', verification: { trust_normalized: true } },
])('retains supported verification when switching $from to $to', ({ from, to, verification }) => {
  const original = source(from, verification);
  expect(selectPlatform(original, to)).toEqual({ ...original, platform: to });
});

it('keeps the verification policy and all other source fields during an unrelated edit', () => {
  const original = source('whatsapp', { trust_normalized: true });
  const onChange = vi.fn();
  render(<SourceFields value={original} onChange={onChange} />);
  fireEvent.change(screen.getByLabelText('Bot username'), { target: { value: 'updated_bot' } });
  expect(onChange).toHaveBeenCalledWith({ ...original, bot_username: 'updated_bot' });
});
