import { ProtocolError } from '../errors.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  positiveInteger,
  text,
  type Output,
} from '../schema.js';

const connector = object({
  platform: text,
  evidence: enumeration('contract_tested', 'upstream_only', 'ingest_only'),
  live_verified: boolean,
  inbound_text: boolean,
  stable_sender_chat: boolean,
  stable_thread: nullable(
    enumeration('telegram_forum_topic', 'discord_channel_thread', 'lark_thread', 'feishu_thread'),
  ),
  reply_to_bot: boolean,
  bot_mentions: boolean,
  inbound_edits: boolean,
  inbound_deletes: boolean,
  verified_actions: boolean,
  attachment_input: enumeration('download_required', 'unavailable'),
  attachment_output: boolean,
  output_text: boolean,
  output_edit: boolean,
  output_progress: boolean,
  output_limit_utf16: nullable(positiveInteger),
  output_format: enumeration('escaped_markdown', 'plain_text', 'unavailable'),
  stages: object({
    management: boolean,
    ingress: boolean,
    allocation: boolean,
    execution: boolean,
    outbound: boolean,
  }),
});
const catalog = object({
  version: integer,
  source_revision: nullable(text),
  connection_validation_available: boolean,
  connectors: array(connector),
});
export type BridgeConnectorCapabilities = Output<typeof connector>;
export type BridgeCapabilityCatalog = Output<typeof catalog>;
/** Unknown contracts remain unavailable; deployment switches never grant authority. */
export function capabilities(value: unknown): BridgeCapabilityCatalog {
  const result = catalog(value);
  if (
    result.version !== 1 ||
    result.connectors.length > 32 ||
    (result.source_revision !== null && !/^[0-9a-f]{40}$/.test(result.source_revision)) ||
    new Set(result.connectors.map((item) => item.platform)).size !== result.connectors.length ||
    result.connectors.some(
      (item) =>
        !/^[a-z][a-z0-9-]{0,63}$/.test(item.platform) ||
        item.output_text !== (item.output_limit_utf16 !== null) ||
        item.output_text !== (item.output_format !== 'unavailable') ||
        ((item.output_edit || item.output_progress) && !item.output_text) ||
        (item.evidence !== 'contract_tested' &&
          (item.inbound_text || item.output_text || Object.values(item.stages).some(Boolean))),
    )
  )
    throw new ProtocolError('bridge capabilities use an unknown or inconsistent contract');
  return result;
}
