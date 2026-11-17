import { resourceActor } from '../resource-access.js';
import { channelUsageSummary } from './channel-usage.js';
import { agentStatus, agentTaskStatus } from '../instances/types.js';
import { enumeration, integer, nullable, object, optional, text, type Output } from '../schema.js';

/** Immutable creation category; later Trigger or channel use does not relabel it. */
export const agentCreationCategory = enumeration(
  'manual',
  'trigger',
  'channel_bridge',
  'legacy_unknown',
);
export type AgentCreationCategory = Output<typeof agentCreationCategory>;
export const agentProvenanceDetails = object({
  created_by: nullable(resourceActor),
  trigger: nullable(
    object({ trigger_id: text, launch_id: text, launching_actor: nullable(resourceActor) }),
  ),
  channel_bridge: nullable(
    object({ bridge_id: text, partition_id: text, generation: integer, source_reference: text }),
  ),
  workspace_profile_index: nullable(integer),
});
export type AgentProvenanceDetails = Output<typeof agentProvenanceDetails>;
/** Details are present only under current resource management authority. */
export const agentProvenance = object({
  agent_id: text,
  created_at: text,
  creation_origin: agentCreationCategory,
  details: optional(agentProvenanceDetails),
});
export type AgentProvenance = Output<typeof agentProvenance>;

/** Safe inventory row; private creation details are available only by point read. */
export const agentProvenanceInventoryItem = object({
  task_status: optional(agentTaskStatus),
  agent_id: text,
  title: optional(text),
  status: agentStatus,
  created_at: text,
  updated_at: text,
  creation_origin: agentCreationCategory,
  channel_usage: optional(channelUsageSummary),
});
export type AgentProvenanceInventoryItem = Output<typeof agentProvenanceInventoryItem>;
