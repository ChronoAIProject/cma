import type { ResolvedConfig } from '../config.js';
import { ProtocolError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { query, read } from '../resources.js';
import {
  boolean,
  integer,
  nullable,
  object,
  positiveInteger,
  text,
  type Output,
} from '../schema.js';

/** Discovery metadata from the original failed turn, never authority. */
export const retainedOutputAvailability = object({ available: boolean });
export type RetainedOutputAvailability = Output<typeof retainedOutputAvailability>;

const descriptor = object({
  id: text,
  agent_id: text,
  turn_sequence: positiveInteger,
  checkpoint_sequence: positiveInteger,
  failure_sequence: positiveInteger,
  failed_at: text,
  byte_length: positiveInteger,
  partial: boolean,
});
const shape = object({
  output: nullable(descriptor),
  offset: integer,
  content: text,
  next: nullable(text),
});
export type RetainedOutput = Output<typeof descriptor>;
export type RetainedOutputPage = Output<typeof shape>;
export interface RetainedOutputOptions extends OperationOptions {
  /** Initial retrieval requires the typed field from history or a response; no prose matching. */
  readonly availability?: RetainedOutputAvailability;
  /** Omit for the first chunk; otherwise pass the previous page's next verbatim. */
  readonly cursor?: string;
}

/** Read-only partial text: never insert this into the durable event sequence or change the run outcome. */
export async function readRetainedOutput(
  config: ResolvedConfig,
  path: string,
  options: RetainedOutputOptions,
  agentId?: string,
): Promise<RetainedOutputPage> {
  if (options.cursor === undefined && options.availability?.available !== true)
    return { output: null, offset: 0, content: '', next: null };
  const page = await read(config, query(path, { cursor: options.cursor }), shape, options);
  const output = page.output;
  const bytes = new TextEncoder().encode(page.content).length;
  if (output === null) {
    if (page.offset !== 0 || bytes !== 0 || page.next !== null || options.cursor !== undefined)
      throw new ProtocolError('Absent retained output contains a continuation or content');
  } else if (
    (agentId !== undefined && output.agent_id !== agentId) ||
    !output.partial ||
    output.byte_length > 1024 * 1024 ||
    bytes === 0 ||
    bytes > 512 * 1024 ||
    page.offset + bytes > output.byte_length ||
    output.turn_sequence > output.checkpoint_sequence ||
    output.checkpoint_sequence >= output.failure_sequence ||
    (page.next !== null) !== page.offset + bytes < output.byte_length ||
    page.next === options.cursor ||
    page.next === '' ||
    (options.cursor === undefined && page.offset !== 0)
  )
    throw new ProtocolError('Retained output identity, bounds or continuation is inconsistent');
  return page;
}
