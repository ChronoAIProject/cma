import { expect, it } from 'vitest';
import { transport } from './resource-fixtures.js';

it('preserves Slack, WhatsApp and X identities in public connector choices', async () => {
  const test = transport();
  const bots = ['slack', 'whatsapp', 'x'].map((platform) => ({
    id: `${platform}-bot`,
    platform,
    active: true,
  }));
  test.answer({ bots, keys: [], routes: [] });
  expect((await test.client.bridges.choices()).bots).toEqual(bots);
});

it('decodes normalized identity and Slack thread facts without granting text or live support', async () => {
  const test = transport();
  const connectors = ['slack', 'whatsapp', 'x'].map((platform) => ({
    platform,
    evidence: 'contract_tested',
    live_verified: false,
    inbound_text: false,
    stable_sender_chat: true,
    stable_thread: platform === 'slack' ? 'slack_thread' : null,
    reply_to_bot: false,
    bot_mentions: false,
    inbound_edits: false,
    inbound_deletes: false,
    verified_actions: false,
    attachment_input: 'unavailable',
    attachment_output: false,
    output_text: false,
    output_edit: false,
    output_progress: false,
    output_limit_utf16: null,
    output_format: 'unavailable',
    stages: {
      management: false,
      ingress: false,
      allocation: false,
      execution: false,
      outbound: false,
    },
  }));
  test.answer({
    version: 1,
    source_revision: '85607255ee0beadca460a350e0aa6d177edd2fd2',
    connection_validation_available: true,
    connectors,
  });
  expect((await test.client.bridges.capabilities()).connectors).toEqual(connectors);
});
