import { check, hash } from '../lib/checks.mjs';
import { BODY_BYTES } from './authority.mjs';

export const TRANSCRIPT_PAGES = 64;
export const TRANSCRIPT_BYTES = BODY_BYTES;
const sequence = (value) => Number.isSafeInteger(value) && value > 0;

export async function readTurn(fixture, prompt, deadline) {
  let request = { limit: 32 }; let through; let bytes = 0; let current; let target;
  const hashes = [];
  for (let page = 0; page < TRANSCRIPT_PAGES; page++) {
    fixture.run.remaining(deadline);
    const result = await fixture.get(`/api/v1/agents/${fixture.agent}/transcript?${new URLSearchParams(request)}`, deadline);
    fixture.run.remaining(deadline);
    const value = result.value; const encoded = JSON.stringify(value);
    bytes += result.bytes ?? Buffer.byteLength(encoded);
    check(bytes <= TRANSCRIPT_BYTES, 'transcript_byte_limit');
    hashes.push(hash(encoded));
    check(value?.agent_id === fixture.agent && Number.isSafeInteger(value.through_seq) && value.through_seq >= 0
      && Array.isArray(value.turns) && value.turns.length <= 1, 'transcript_page_identity');
    through ??= value.through_seq;
    check(value.through_seq === through, 'transcript_high_water_changed');
    if (!value.turns.length) {
      check(page === 0 && !value.next, 'transcript_empty_continuation');
      return { evidenceHash: hash(hashes.join(':')) };
    }
    const { messages, agent_messages: answers, ...header } = value.turns[0];
    const turn = header.turn_sequence;
    check(sequence(turn) && turn <= through && (!request.turn || request.turn === turn)
      && typeof header.owner_message === 'string' && Array.isArray(messages) && Array.isArray(answers),
    'transcript_turn_identity');
    const after = request.after ?? turn - 1;
    const next = value.next;
    let end = through;
    if (next) {
      check(sequence(next.turn) && next.turn >= turn && next.turn <= through
        && Number.isSafeInteger(next.after) && next.after > after && next.after < through
        && next.through === through && next.limit === request.limit, 'transcript_continuation_invalid');
      check(next.turn === turn ? next.after === after + request.limit : next.after === next.turn - 1,
        'transcript_continuation_stale');
      end = next.after;
    }
    check(end > after && end <= after + request.limit, 'transcript_window_invalid');
    if (current?.header.turn_sequence === turn) {
      check(JSON.stringify(current.header) === JSON.stringify(header), 'transcript_header_changed');
    } else {
      current = { header, lastMessage: 0 };
      if (header.owner_message === prompt) {
        check(!target, 'duplicate_durable_input');
        target = { ...header, agent_messages: [] };
      }
    }
    const assistant = [];
    for (const message of messages) {
      const queuedOwner = after === turn - 1 && message.role === 'user' && message.sequence < turn;
      check(sequence(message.sequence) && message.sequence > current.lastMessage && message.sequence <= end
        && (message.sequence > after || queuedOwner) && ['user', 'assistant'].includes(message.role)
        && typeof message.content === 'string', 'transcript_message_invalid');
      current.lastMessage = message.sequence;
      if (message.role === 'assistant') assistant.push(message.content);
    }
    check(JSON.stringify(assistant) === JSON.stringify(answers), 'transcript_messages_disagree');
    if (target?.turn_sequence === turn) target.agent_messages.push(...assistant);
    if (!next) return { turn: target, evidenceHash: hash(hashes.join(':')) };
    request = { turn: next.turn, after: next.after, through: next.through, limit: next.limit };
  }
  check(false, 'transcript_page_limit');
}
