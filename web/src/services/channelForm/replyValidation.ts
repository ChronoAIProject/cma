import { canonical } from './relayAuthoring';
import { object, records, text, type Fields } from './model';

export const maxAckDelay = 60_000;
export const ackDelayHint =
  'Use 0 to disable acknowledgements, or a whole number from 1 to 60,000 ms (60 seconds).';
export function ackDelayMessage(value: unknown, retained?: unknown) {
  if (value === retained && retained !== undefined) return undefined;
  return typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= maxAckDelay
    ? undefined
    : ackDelayHint;
}
export function supportsVerifiedThreads(value: Fields) {
  const sources = records(value.sources);
  const source = sources[0];
  return (
    sources.length === 1 &&
    source?.type === 'nyxid_relay' &&
    ['lark', 'feishu'].includes(text(source.platform)) &&
    source.trust_normalized !== true &&
    /^ou_[A-Za-z0-9_.-]{1,125}$/.test(text(source.bot_id))
  );
}
export function validateReplyFields(next: Fields, previous?: Fields) {
  const reply = object(next.reply);
  const old = object(previous?.reply);
  const message = ackDelayMessage(reply.ack_after_ms ?? 0, old.ack_after_ms);
  if (message) throw new Error(`/reply/ack_after_ms: ${message}`);
  if (
    reply.thread_contract != null &&
    !supportsVerifiedThreads(next) &&
    !(
      canonical(reply.thread_contract) === canonical(old.thread_contract) &&
      canonical(next.sources) === canonical(previous?.sources)
    )
  )
    throw new Error(
      '/reply/thread_contract: Thread following requires one verified NyxID Lark or Feishu producer with a bot ID.',
    );
}
