import { expect, it } from 'vitest';
import { ProtocolError } from '../src/index.js';
import { transport } from './resource-fixtures.js';

const empty = {
  version: 1,
  source_revision: null,
  connection_validation_available: false,
  connectors: [],
};
it('reads capabilities in exact owner scope and keeps old validator defaults unavailable', async () => {
  const test = transport();
  test.answer({ ...empty, future_field: true });
  expect(await test.client.bridges.capabilities({ organization: 'org-one' })).toEqual(empty);
  expect(new URL(test.calls[0]!.url).pathname).toBe('/api/v1/bridge-connectors/capabilities');
  expect(new URL(test.calls[0]!.url).searchParams.get('organization')).toBe('org-one');
  expect(test.calls[0]!.init.body).toBeUndefined();
});
it('rejects unknown versions and unsubstantiated capability shapes', async () => {
  const test = transport();
  for (const value of [
    { ...empty, version: 2 },
    { ...empty, source_revision: 'unverified' },
    { ...empty, connectors: [{ platform: 'telegram', inbound_text: true }] },
  ]) {
    test.answer(value);
    await expect(test.client.bridges.capabilities()).rejects.toBeInstanceOf(ProtocolError);
  }
});

it('validates a complete connector projection without granting support to inconsistent metadata', async () => {
  const test = transport();
  const connector = {
    platform: 'telegram',
    evidence: 'contract_tested',
    live_verified: false,
    inbound_text: true,
    stable_sender_chat: true,
    stable_thread: 'telegram_forum_topic',
    reply_to_bot: true,
    bot_mentions: false,
    inbound_edits: false,
    inbound_deletes: false,
    verified_actions: false,
    attachment_input: 'download_required',
    attachment_output: false,
    output_text: true,
    output_edit: false,
    output_progress: true,
    output_limit_utf16: 4000,
    output_format: 'escaped_markdown',
    stages: {
      management: true,
      ingress: false,
      allocation: false,
      execution: false,
      outbound: false,
    },
  };
  const catalog = {
    ...empty,
    source_revision: '85607255ee0beadca460a350e0aa6d177edd2fd2',
    connectors: [connector],
  };
  test.answer(catalog);
  expect((await test.client.bridges.capabilities()).connectors).toEqual([connector]);
  for (const changed of [
    { ...connector, output_text: false, output_progress: false, output_format: 'unavailable' },
    { ...connector, evidence: 'upstream_only' },
    { ...connector, output_limit_utf16: null },
  ]) {
    test.answer({ ...catalog, connectors: [changed] });
    await expect(test.client.bridges.capabilities()).rejects.toBeInstanceOf(ProtocolError);
  }
  test.answer({ ...catalog, connectors: [connector, connector] });
  await expect(test.client.bridges.capabilities()).rejects.toBeInstanceOf(ProtocolError);
});

it('preserves telegram-new as a distinct selectable platform', async () => {
  const test = transport();
  test.answer({
    bots: [{ id: 'bot', platform: 'telegram-new', active: true }],
    keys: [],
    routes: [],
  });
  expect((await test.client.bridges.choices()).bots[0]!.platform).toBe('telegram-new');
});

it('preserves placement evidence and refuses unknown, duplicated or malformed placement catalogs', async () => {
  const test = transport();
  const placements = [
    { kind: 'workspace_per_partition', availability: 'continuity_unverified' },
    { kind: 'worktree_agent_per_partition', availability: 'runtime_unavailable' },
  ];
  test.answer({ ...empty, placements });
  expect((await test.client.bridges.capabilities()).placements).toEqual(placements);
  for (const invalid of [
    null,
    [placements[0], placements[0]],
    [{ kind: 'future_placement', availability: 'available' }],
    [{ kind: 'workspace_per_partition', availability: 'probably_ready' }],
    [{ kind: 'workspace_per_partition' }],
  ]) {
    test.answer({ ...empty, placements: invalid });
    await expect(test.client.bridges.capabilities()).rejects.toBeInstanceOf(ProtocolError);
  }
  test.answer(empty);
  expect((await test.client.bridges.capabilities()).placements).toBeUndefined();
});
