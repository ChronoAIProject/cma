import { retainedOutputAvailability } from '../agents/retained-output.js';
import { taskAcceptance } from '../task-acceptance.js';
import {
  agentStatus,
  failure,
  sandboxStatus,
  workspaceStatus,
  workspaceSource,
  worktreeStatus,
} from '../instances/types.js';
import { triggerQuickMessage } from '../trigger-authoring/types.js';
import {
  permissionLevel,
  profileReference,
  skillReference,
  thinkingEffort,
} from '../profiles/types.js';
import {
  array,
  boolean,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  positiveInteger,
  tagged,
  text,
  variant,
  type Output,
} from '../schema.js';

export const firstMessageDelivery = enumeration(
  'queued',
  'dispatch_fenced',
  'write_confirmed',
  'execution_observed',
  'completed',
  'failed',
  'recovery_required',
);
export type FirstMessageDelivery = Output<typeof firstMessageDelivery>;
export const firstMessageState = object({
  state: firstMessageDelivery,
  purpose: optional(enumeration('setup', 'task')),
  turn_sequence: optional(integer),
  queued_at: text,
  completed_at: optional(text),
  failed_at: optional(text),
});
export type FirstMessageState = Output<typeof firstMessageState>;
export const agentProfileSummary = object({
  id: text,
  revision: positiveInteger,
  digest: text,
  model: optional(text),
  thinking_effort: thinkingEffort,
  permission_level: permissionLevel,
  skills: array(skillReference),
});
export type AgentProfileSummary = Output<typeof agentProfileSummary>;
const instanceProfileReference = object({ id: text, revision: positiveInteger, digest: text });
export const agentWorktree = object({
  id: text,
  name: text,
  path: text,
  branch: text,
  base_commit: text,
  status: worktreeStatus,
});
export type AgentWorktree = Output<typeof agentWorktree>;
export const agentWorkspace = object({
  id: text,
  name: text,
  path: text,
  status: workspaceStatus,
  profile: instanceProfileReference,
  resolved_commit: optional(text),
  source: optional(workspaceSource),
});
export type AgentWorkspace = Output<typeof agentWorkspace>;
export const agentSandbox = object({
  id: text,
  name: text,
  status: sandboxStatus,
  profile: object({
    id: text,
    revision: positiveInteger,
    digest: text,
    readable: optional(boolean),
  }),
});
export type AgentSandbox = Output<typeof agentSandbox>;
export const agentRuntimeMetadata = object({
  working_directory: optional(text),
  description: optional(text),
  description_source: optional(enumeration('provider_title', 'prompt_preview')),
  description_is_setup_preview: optional(boolean),
});
export type AgentRuntimeMetadata = Output<typeof agentRuntimeMetadata>;
export const agentTurnUsage = object({
  turn_sequence: integer,
  input_tokens: integer,
  output_tokens: integer,
});
export type AgentTurnUsage = Output<typeof agentTurnUsage>;
export const agentSummary = object({
  runtime_metadata: optional(agentRuntimeMetadata),
  owner_title: optional(text),
  id: text,
  sandbox_id: text,
  workspace_id: text,
  worktree_id: text,
  status: agentStatus,
  profile: agentProfileSummary,
  title: optional(text),
  created_at: text,
  updated_at: text,
});
export type AgentSummary = Output<typeof agentSummary>;
export const agent = object({
  task_acceptance: optional(taskAcceptance),
  callback_url: optional(nullable(text)),
  runtime_metadata: optional(agentRuntimeMetadata),
  owner_title: optional(text),
  id: text,
  status: agentStatus,
  profile: agentProfileSummary,
  worktree: agentWorktree,
  workspace: agentWorkspace,
  sandbox: agentSandbox,
  desktop: optional(boolean),
  bootstrap: optional(object({ operation_id: text, sandbox_id: text, workspace_id: text })),
  conforms_to: array(text),
  reach: array(text),
  title: optional(text),
  paused_notice: optional(text),
  first_message: optional(firstMessageState),
  failure: optional(failure),
  stop_reason: optional(text),
  restart_available: optional(boolean),
  created_at: text,
  updated_at: text,
  ready_at: optional(text),
  stopped_at: optional(text),
  deleted_at: optional(text),
});
export type Agent = Output<typeof agent>;

export const agentSource = object({
  agent_id: text,
  origin: tagged({
    manual: variant('manual', {}),
    workspace_profile: variant('workspace_profile', {}),
    trigger: variant('trigger', { trigger_id: text, launch_id: text }),
  }),
  trigger: optional(
    object({
      public_id: text,
      title: text,
      revision: positiveInteger,
      quick_messages: array(triggerQuickMessage),
    }),
  ),
});
export type AgentSource = Output<typeof agentSource>;

export const createAgentRequest = object({
  agent_profile: profileReference,
  title: optional(text),
  first_message: optional(text),
});
export type CreateAgentRequest = Output<typeof createAgentRequest>;
export const agentTextRequest = object({ text });
export type AgentTextRequest = Output<typeof agentTextRequest>;
export const approvalDecision = enumeration('allow', 'deny', 'abort');
export type ApprovalDecision = Output<typeof approvalDecision>;
export const approvalDecisionRequest = object({ decision: approvalDecision });
export type ApprovalDecisionRequest = Output<typeof approvalDecisionRequest>;
export const stopAgentRequest = object({ reason: optional(enumeration('owner')) });
export type StopAgentRequest = Output<typeof stopAgentRequest>;

export const transcriptOutcome = tagged({
  active: variant('active', {}),
  completed: variant('completed', { at: text }),
  failed: variant('failed', { at: text, failure }),
  interrupted: variant('interrupted', { at: text }),
});
export type TranscriptOutcome = Output<typeof transcriptOutcome>;
export const transcriptMessage = object({
  sequence: positiveInteger,
  role: enumeration('user', 'assistant'),
  content: text,
  appended_at: text,
});
export type TranscriptMessage = Output<typeof transcriptMessage>;
export const transcriptTurn = object({
  retained_output: optional(retainedOutputAvailability),
  turn_sequence: integer,
  started_at: text,
  owner_message: text,
  agent_messages: array(text),
  messages: array(transcriptMessage),
  outcome: transcriptOutcome,
});
export type TranscriptTurn = Output<typeof transcriptTurn>;
export const agentTranscript = object({
  agent_id: text,
  through_seq: integer,
  turns: array(transcriptTurn),
});
export type AgentTranscript = Output<typeof agentTranscript>;
export const agentUsage = object({
  current_turn: optional(agentTurnUsage),
  agent_id: text,
  through_seq: integer,
  input_tokens: integer,
  output_tokens: integer,
  context_used: integer,
  context_window: optional(integer),
});
export type AgentUsage = Output<typeof agentUsage>;
export const agentResources = object({
  agent_id: text,
  sandbox_id: text,
  uptime_ms: integer,
  cpu_usage_usec: nullable(integer),
  cpu_millis_limit: nullable(integer),
  memory_bytes_used: nullable(integer),
  memory_bytes_limit: nullable(integer),
  memory_bytes_peak: nullable(integer),
  stall_permille: nullable(integer),
  ceiling_hits: nullable(integer),
  allocation_failures: nullable(integer),
  kills: nullable(integer),
  received_bytes: nullable(integer),
  sent_bytes: nullable(integer),
});
export type AgentResources = Output<typeof agentResources>;
