import { ProtocolError } from './errors.js';
import { positiveInteger, record } from './schema.js';

export interface AnswerContext {
  readonly turn_sequence: number;
  readonly answer_sequence: number;
}

export function answerContext(value: unknown): AnswerContext {
  const source = record(value);
  if (Object.keys(source).length !== 2)
    throw new ProtocolError('answer context requires an exact turn and answer');
  const turn_sequence = positiveInteger(source.turn_sequence);
  const answer_sequence = positiveInteger(source.answer_sequence);
  if (answer_sequence <= turn_sequence) throw new ProtocolError('answer must follow its turn');
  return { turn_sequence, answer_sequence };
}
