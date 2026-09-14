import { firstMessageDelivery } from '../agents/types.js';
import { sandboxStatus, workspaceStatus, worktreeStatus } from '../instances/types.js';
import {
  array,
  boolean,
  dictionary,
  enumeration,
  integer,
  nullable,
  object,
  optional,
  text,
  type Output,
} from '../schema.js';

export const agentSandboxSnapshot = object({
  threadId: text,
  sandbox: object({
    id: text,
    name: text,
    status: sandboxStatus,
    paused: boolean,
    pausedNotice: nullable(text),
    rotation: object({ rotations: integer, rotated_at: optional(text) }),
  }),
  workspace: object({ id: text, name: text, path: text, status: workspaceStatus }),
  worktree: object({ id: text, name: text, path: text, status: worktreeStatus }),
  firstMessage: nullable(
    object({
      state: firstMessageDelivery,
      attempt: integer,
      queuedAt: text,
      completedAt: nullable(text),
      failedAt: nullable(text),
    }),
  ),
});
export type AgentSandboxSnapshot = Output<typeof agentSandboxSnapshot>;

export const aguiCapabilities = object({
  protocol: enumeration('ag-ui'),
  transports: array(enumeration('sse')),
  events: dictionary(boolean),
  features: object({
    transport: object({
      streaming: boolean,
      resumable: boolean,
      websocket: boolean,
      pushNotifications: boolean,
    }),
    state: object({ snapshots: boolean, deltas: boolean, persistentState: boolean }),
    humanInTheLoop: object({
      supported: boolean,
      approvals: boolean,
      interrupts: boolean,
      approveWithEdits: boolean,
      interventions: boolean,
    }),
    execution: object({ codeExecution: boolean, sandboxed: boolean }),
    custom: object({
      'cma:terminal': boolean,
      'cma:usage': boolean,
      'cma:liveFrames': boolean,
      'cma:sandbox': object({
        snapshot: boolean,
        status: boolean,
        workspace: boolean,
        worktree: boolean,
        pausedNotice: boolean,
        firstMessageDelivery: boolean,
        mutations: boolean,
      }),
    }),
  }),
  conformsTo: array(text),
});
export type AguiCapabilities = Output<typeof aguiCapabilities>;
