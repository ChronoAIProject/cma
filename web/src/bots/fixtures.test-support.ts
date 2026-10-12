import { vi } from 'vitest';
import type { Bot } from '@chronoai/cma-sdk';
import type { BotClient } from './model';

export const bot: Bot = {
  bot_id: 'bot_fixture',
  owner: 'owner',
  name: 'Research assistant',
  nyxid_bot_id: 'external-fixture',
  profile: { id: 'agp_fixture', revision: 1 },
  provider: 'cma',
  state: 'active',
  failure: null,
  operation: null,
  channel: {
    channel_id: 'channel-fixture',
    record_id: 'record-fixture',
    version: 1,
    binding_id: 'binding-fixture',
    level: 'L2',
    callback_url: 'https://gateway.example/v1/callbacks/opaque',
    event_url: 'https://gateway.example/v1/channels/opaque/events',
  },
  credentials_expire_at: '2026-09-24T00:00:00Z',
  created_at: '2026-09-23T00:00:00Z',
  updated_at: '2026-09-23T00:00:00Z',
  last_test: null,
};
export function botClient(overrides: Partial<BotClient['bots']> = {}) {
  return {
    bots: {
      list: vi.fn(async () => ({ items: [bot], next_after: null })),
      get: vi.fn(async () => bot),
      available: vi.fn(async () => [
        { id: 'external-fixture', name: 'Telegram research', platform: 'telegram' },
      ]),
      create: vi.fn(async () => ({ ...bot, state: 'creating' as const })),
      test: vi.fn(async () => ({
        accepted_after_ms: 42,
        first_text: 'Ready to help.',
        level: 'L2',
        tested_at: '2026-09-23T00:00:00Z',
      })),
      disable: vi.fn(async () => ({ ...bot, state: 'disabled' as const })),
      enable: vi.fn(async () => bot),
      rotateKeys: vi.fn(async () => bot),
      delete: vi.fn(async () => ({ ...bot, state: 'deleting' as const })),
      ...overrides,
    },
  };
}
