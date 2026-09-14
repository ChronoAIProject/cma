import { ProtocolError } from '../errors.js';
import { agentStatus } from '../instances/types.js';
import { enumeration, object, optional, record, text, type Output } from '../schema.js';
import { agent, type Agent } from './types.js';

/** Safe detail for a reader whose authority does not include personal ownership. */
export const sharedAgent = object({
  access: enumeration('shared'),
  id: text,
  status: agentStatus,
  title: optional(text),
  model: optional(text),
  created_at: text,
  updated_at: text,
  ready_at: optional(text),
  stopped_at: optional(text),
  deleted_at: optional(text),
});
export type SharedAgent = Output<typeof sharedAgent>;
export type AccessibleAgent = (Agent & { readonly access?: never }) | SharedAgent;

export function accessibleAgent(value: unknown): AccessibleAgent {
  const source = record(value);
  if (source.access === undefined) return agent(value);
  if (source.access === 'shared') return sharedAgent(value);
  throw new ProtocolError('unknown Agent access discriminator');
}
