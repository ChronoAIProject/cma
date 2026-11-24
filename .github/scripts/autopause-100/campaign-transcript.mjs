import {isDeepStrictEqual} from 'node:util';
import {check} from '../scale-tests/lib/checks.mjs';

const PAGE_SIZE = 32;
const MAX_PAGES = 512;
const MAX_TURNS = 120;
const positive = value => Number.isSafeInteger(value) && value > 0;
const afterTurnStart = (after, turn) => Number.isSafeInteger(after) && after >= turn - 1;

export async function readCampaignTranscript(agentId, read) {
  const turns = [];
  let cursor;
  let through;
  for (let page = 0; page < MAX_PAGES; page += 1) {
    const query = new URLSearchParams({limit: String(PAGE_SIZE)});
    if (cursor) {
      for (const field of ['turn', 'after', 'through']) query.set(field, String(cursor[field]));
    }
    const result = await read(query);
    check(result?.agent_id === agentId && positive(result.through_seq)
      && (through === undefined || through === result.through_seq)
      && Array.isArray(result.turns) && result.turns.length === 1, 'campaign_transcript_incomplete');
    through = result.through_seq;
    const fragment = result.turns[0];
    const {messages, agent_messages: assistant, ...header} = fragment;
    const sequence = header.turn_sequence;
    check(positive(sequence) && sequence <= through && (!cursor || cursor.turn === sequence)
      && Array.isArray(messages) && Array.isArray(assistant)
      && isDeepStrictEqual(messages.filter(message => message?.role === 'assistant').map(message => message.content),
        assistant), 'campaign_transcript_fragment_invalid');
    const previous = turns.at(-1);
    const sameTurn = previous?.header.turn_sequence === sequence;
    let lastMessage = sameTurn ? (previous.messages.at(-1)?.sequence ?? cursor.after) : 0;
    for (const message of messages) {
      check(positive(message?.sequence) && message.sequence > lastMessage && message.sequence <= through
        && (!sameTurn || message.sequence > cursor.after)
        && ['user', 'assistant'].includes(message.role) && typeof message.content === 'string',
      'campaign_transcript_message_invalid');
      lastMessage = message.sequence;
    }
    if (sameTurn) {
      check(isDeepStrictEqual(previous.header, header), 'campaign_transcript_turn_changed');
      previous.messages.push(...messages);
      previous.agent_messages.push(...assistant);
    } else {
      check((!previous || sequence > previous.header.turn_sequence)
        && turns.length < MAX_TURNS, 'campaign_transcript_turn_invalid');
      turns.push({header, messages: [...messages], agent_messages: [...assistant]});
    }

    const next = result.next;
    if (next === undefined || next === null) {
      return turns.map(({header, messages, agent_messages}) => ({...header, messages, agent_messages}));
    }
    check(next.through === through && next.limit === PAGE_SIZE
      && positive(next.turn) && next.turn >= sequence
      && afterTurnStart(next.after, next.turn) && next.after < through
      && (next.turn === sequence
        ? next.after > (cursor?.after ?? sequence - 1) && next.after >= lastMessage
        : next.after === next.turn - 1), 'campaign_transcript_cursor_invalid');
    cursor = next;
  }
  check(false, 'campaign_transcript_page_limit');
}
