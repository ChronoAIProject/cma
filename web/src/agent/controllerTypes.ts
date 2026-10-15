import type { AgentSummary } from '../api/types';
import type { AgentLoadLineage, ReadyAgent } from './loadAgent';
import type {
  useSandboxInput,
  SandboxInputAvailability,
  SandboxResourceAvailability,
} from './sandboxInput';
import type { QueuedMessageControl } from './transport/queuedStatus';
import type { QueuedInputsControl } from './transport/queuedInputsStatus';

export type MessageAcceptance =
  { readonly commandId: string; readonly responseId: string } | { readonly runId: string };

export type AgentResourceState =
  | { readonly status: 'idle' }
  | { readonly status: 'loading' }
  | { readonly status: 'error'; readonly message: string }
  | ({ readonly status: 'ready' } & ReadyAgent);

export type AgentCommand = 'send' | 'interrupt' | 'approval' | 'stop' | 'delete';
export type ApprovalDecision = 'allow' | 'deny' | 'abort';

export interface AgentController {
  readonly queuedMessage?: QueuedMessageControl;
  readonly queuedInputs?: QueuedInputsControl;
  readonly pendingMessage?: string;
  readonly acceptedMessage?: string;
  readonly acceptedSteeringMessage?: string;
  readonly pendingControl?: boolean;
  readonly controlNotice?: string;
  readonly controlsUnavailable?: boolean;
  readonly recoverPending?: () => Promise<void>;
  readonly state: AgentResourceState;
  readonly inputAvailability: SandboxInputAvailability;
  readonly resourceAvailability: SandboxResourceAvailability;
  readonly progressObservation?: ReturnType<typeof useSandboxInput>['progressObservation'];
  readonly pending?: AgentCommand;
  readonly actionError?: string;
  readonly reload: () => void;
  readonly clearActionError: () => void;
  readonly send: (
    content: string,
    onAccepted?: (receipt?: MessageAcceptance) => void,
    onReceiptObserved?: (receipt: MessageAcceptance) => void,
    onRefused?: () => void,
  ) => Promise<void>;
  readonly interrupt: () => Promise<void>;
  readonly steer?: AgentController['send'];
  readonly approve: (approvalId: string, decision: ApprovalDecision) => Promise<void>;
  readonly stop: () => Promise<void>;
  readonly delete: () => Promise<void>;
}

export interface AgentControllerOptions {
  readonly viewerId?: string;
  readonly suspended?: boolean;
  readonly authorizedLineage?: AgentLoadLineage;
  readonly summary?: AgentSummary;
  readonly onHierarchyChange?: () => void;
  readonly onDeleted?: () => void;
}
