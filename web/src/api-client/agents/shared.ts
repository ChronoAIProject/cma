import { ProtocolError } from '../errors.js';
import { agentStatus } from '../instances/types.js';
import {
  enumeration,
  nullable,
  object,
  optional,
  record,
  tagged,
  text,
  variant,
  type Output,
} from '../schema.js';
import {
  agent,
  agentSource,
  agentSummary,
  type Agent,
  type AgentSource,
  type AgentSummary,
} from './types.js';

/** Safe detail for a reader whose authority does not include personal ownership. */
export const sharedAgent = object({
  callback_url: optional(nullable(text)),
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

export type AccessibleAgentSummary = (AgentSummary & { readonly access?: never }) | SharedAgent;
export function accessibleAgentSummary(value: unknown): AccessibleAgentSummary {
  const source = record(value);
  if (source.access === undefined) return agentSummary(value);
  if (source.access === 'shared') return sharedAgent(value);
  throw new ProtocolError('unknown Agent access discriminator');
}

/** Creation category without private Launch/Trigger identities or shortcuts. */
export const sharedAgentSource = object({
  access: enumeration('shared'),
  agent_id: text,
  origin: tagged({
    manual: variant('manual', {}),
    workspace_profile: variant('workspace_profile', {}),
    trigger: variant('trigger', {}),
  }),
});
export type SharedAgentSource = Output<typeof sharedAgentSource>;
export type AccessibleAgentSource = (AgentSource & { readonly access?: never }) | SharedAgentSource;
export function accessibleAgentSource(value: unknown): AccessibleAgentSource {
  const source = record(value);
  if (source.access === undefined) return agentSource(value);
  if (source.access === 'shared') return sharedAgentSource(value);
  throw new ProtocolError('unknown Agent source access discriminator');
}
