import { describe, expect, it, vi } from 'vitest';
import { Cma } from '../src/client.js';
import { resolveConfig } from '../src/config.js';
import { ProtocolError, ValidationError } from '../src/errors.js';
import {
  changeChatSettings,
  readChatSettings,
  toStoredChatSettings,
  type ChatSettings,
} from '../src/settings.js';

/** @vitest-environment node */

const CHAT_WIRE = {
  enabled: true,
  workspace: {
    repository_url: 'https://example.test/team/repository.git',
    branch: 'development',
  },
  agent: {
    model: 'reasoning-model',
    reasoning_effort: 'x_high',
    instructions: 'Read the repository guidance.',
    record_tool_bodies: false,
  },
  message_prefixes: ['/formalize', '/check'],
  inactivity: { pause_after_seconds: 1_200 },
  setup: { commands: ['tool install', 'tool verify'] },
  host: {
    size: { cpu_millis: 2_000, memory_mib: 4_096 },
    max_threads: 12,
  },
} as const;

const CHAT: ChatSettings = {
  enabled: true,
  workspace: {
    repositoryUrl: 'https://example.test/team/repository.git',
    branch: 'development',
  },
  agent: {
    model: 'reasoning-model',
    reasoningEffort: 'x_high',
    instructions: 'Read the repository guidance.',
    recordToolBodies: false,
  },
  messagePrefixes: ['/formalize', '/check'],
  inactivity: { pauseAfterSeconds: 1_200 },
  setup: { commands: ['tool install', 'tool verify'] },
  host: { size: { cpuMillis: 2_000, memoryMib: 4_096 }, maxThreads: 12 },
};

function response(chat: unknown = CHAT_WIRE): Response {
  return new Response(
    JSON.stringify({
      revision: 7,
      settings: { chat },
      previous: { chat: null },
    }),
    { status: 200, headers: { 'content-type': 'application/json' } },
  );
}

describe('chat settings wire shape', () => {
  it('round-trips every field and preserves absent previous state', () => {
    expect(
      toStoredChatSettings({
        revision: 7,
        settings: { chat: CHAT_WIRE, unrelated_future_setting: true },
        previous: { chat: null },
      }),
    ).toEqual({ revision: 7, chat: CHAT, previousChat: null });

    expect(toStoredChatSettings({ revision: 0, settings: {} })).toEqual({
      revision: 0,
      chat: null,
      previousChat: undefined,
    });
  });

  it('refuses malformed known fields rather than inventing defaults', () => {
    expect(() =>
      toStoredChatSettings({
        revision: 7,
        settings: { chat: { ...CHAT_WIRE, enabled: 'yes' } },
      }),
    ).toThrow(ProtocolError);
    expect(() =>
      toStoredChatSettings({
        revision: 7,
        settings: {
          chat: { ...CHAT_WIRE, inactivity: { pause_after_seconds: 59 } },
        },
      }),
    ).toThrow(ProtocolError);
  });
});

describe('chat settings operations', () => {
  it('reads and changes only chat through the public admin route', async () => {
    const fetch = vi.fn().mockImplementation(() => Promise.resolve(response()));
    const config = resolveConfig({ fetch: fetch as typeof globalThis.fetch, cookies: false });

    await expect(readChatSettings(config)).resolves.toEqual({
      revision: 7,
      chat: CHAT,
      previousChat: null,
    });
    await expect(changeChatSettings(config, 6, CHAT)).resolves.toMatchObject({ revision: 7 });

    expect(fetch.mock.calls.map(([url, init]) => [String(url), init?.method ?? 'GET'])).toEqual([
      ['/api/v1/admin/settings', 'GET'],
      ['/api/v1/admin/settings', 'PATCH'],
    ]);
    expect(JSON.parse(String(fetch.mock.calls[1]?.[1]?.body))).toEqual({
      expected_revision: 6,
      chat: CHAT_WIRE,
    });
    expect(fetch.mock.calls.every(([, init]) => init?.credentials === 'omit')).toBe(true);
  });

  it('supports removing chat and validates every dangerous boundary before fetch', async () => {
    const fetch = vi.fn().mockImplementation(() => Promise.resolve(response(null)));
    const config = resolveConfig({ fetch: fetch as typeof globalThis.fetch });
    await expect(changeChatSettings(config, 7, null)).resolves.toMatchObject({ chat: null });
    expect(JSON.parse(String(fetch.mock.calls[0]?.[1]?.body))).toEqual({
      expected_revision: 7,
      chat: null,
    });

    const invalid = [
      { ...CHAT, workspace: { ...CHAT.workspace, repositoryUrl: 'http://example.test/repo' } },
      { ...CHAT, workspace: { ...CHAT.workspace, branch: 'two words' } },
      { ...CHAT, workspace: { ...CHAT.workspace, branch: 'é'.repeat(101) } },
      { ...CHAT, messagePrefixes: Array.from({ length: 9 }, () => '/prefix') },
      { ...CHAT, messagePrefixes: ['💡'.repeat(65)] },
      { ...CHAT, inactivity: { pauseAfterSeconds: 59 } },
      { ...CHAT, setup: { commands: Array.from({ length: 17 }, () => '') } },
      { ...CHAT, setup: { commands: ['💡'.repeat(1025)] } },
      { ...CHAT, host: { ...CHAT.host, maxThreads: 51 } },
      { ...CHAT, host: { ...CHAT.host, size: { cpuMillis: 0, memoryMib: 512 } } },
    ];
    for (const chat of invalid) {
      await expect(changeChatSettings(config, 7, chat)).rejects.toBeInstanceOf(ValidationError);
    }
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it('is exposed by the configured client', async () => {
    const fetch = vi.fn().mockImplementation(() => Promise.resolve(response()));
    const cma = new Cma({ fetch: fetch as typeof globalThis.fetch });
    await expect(cma.readChatSettings()).resolves.toMatchObject({ revision: 7 });
    await expect(cma.changeChatSettings(6, CHAT)).resolves.toMatchObject({ chat: CHAT });
  });
});
