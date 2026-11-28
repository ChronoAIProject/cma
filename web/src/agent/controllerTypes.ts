import type { ResponseControlAvailability } from './controlReadiness';
import type { StopRequestState } from './transport/controlStatus';
import type { AgentSummary } from '../api/types';
import type { AnswerContext } from '@cma/client';
import type { LegacySuggestionRecord } from './transport/legacySuggestionJournal';
import type { InputRecord } from './transport/journalRecord';
import type { AgentLoadLineage, ReadyAgent } from './loadAgent';
import type {
  useSandboxInput,
  SandboxInputAvailability,
  SandboxResourceAvailability,
} from './sandboxInput';
import type { QueuedMessageControl } from './transport/queuedStatus';
import type { QueuedInputsControl } from './transport/queuedInputsStatus';
import type { SelectedRetiredAgent } from '../useDeletionNoticeReconciliation';
import type { RetainedAgentRead } from './retainedHistory';

export type MessageAcceptance =
  { readonly commandId: string; readonly responseId: string } | { readonly runId: string };

export type AgentResourceState =
  | { readonly status: 'idle' }
  | { readonly status: 'loading' }
  | {
      readonly status: 'error';
      readonly message: string;
      readonly retryAt?: number;
      readonly workspaceUnavailable?: boolean;
    }
  | ({ readonly status: 'ready' } & ReadyAgent);

export type AgentCommand = 'send' | 'interrupt' | 'approval' | 'stop' | 'delete' | 'restart';
export type ApprovalDecision = 'allow' | 'deny' | 'abort';

export interface AgentController {
  readonly retained?: RetainedAgentRead;
  readonly readSuggestionRequest?: () => LegacySuggestionRecord | InputRecord | null | undefined;
  readonly applyAgentTitle?: (title: string) => void;
  readonly queuedMessage?: QueuedMessageControl;
  readonly queuedInputs?: QueuedInputsControl;
  readonly pendingMessage?: string;
  readonly inputCoolingDown?: boolean;
  readonly checkingInputReceipt?: boolean;
  readonly acceptedMessage?: string;
  readonly acceptedSteeringMessage?: string;
  readonly acceptedInterruptionMessage?: string;
  readonly pendingControl?: boolean;
  readonly controlNotice?: string;
  readonly controlsUnavailable?: boolean;
  readonly responseControls?: ResponseControlAvailability;
  readonly stopRequest?: StopRequestState;
  readonly controlBaselineReady?: boolean;
  readonly recoverPending?: () => Promise<void>;
  readonly state: AgentResourceState;
  readonly inputAvailability: SandboxInputAvailability;
  readonly resourceAvailability: SandboxResourceAvailability;
  readonly progressObservation?: ReturnType<typeof useSandboxInput>['progressObservation'];
  readonly pending?: AgentCommand;
  readonly initialSendPending?: boolean;
  readonly actionError?: string;
  readonly streamError?: string;
  readonly reload: () => void;
  readonly refreshDetail: () => void;
  readonly clearActionError: () => void;
  readonly send: (
    content: string,
    onAccepted?: (receipt?: MessageAcceptance) => void,
    onReceiptObserved?: (receipt: MessageAcceptance) => void,
    onRefused?: (reason?: 'answer_changed' | 'suggestions_unavailable') => void,
    answerContext?: AnswerContext,
  ) => Promise<void>;
  readonly interrupt: () => Promise<void>;
  readonly interruptForControl?: () => Promise<void>;
  readonly steer?: AgentController['send'];
  readonly interruptWithMessage?: (
    content: string,
    onAccepted?: (receipt?: MessageAcceptance) => void,
    onReceiptObserved?: (receipt: MessageAcceptance) => void,
    onRefused?: () => void,
    onOrdinaryInput?: () => void,
  ) => Promise<void>;
  readonly approve: (approvalId: string, decision: ApprovalDecision) => Promise<void>;
  readonly stop: () => Promise<void>;
  readonly restart?: () => Promise<void>;
  readonly delete: () => Promise<void>;
}

export interface AgentControllerOptions {
  readonly selectedRetired?: SelectedRetiredAgent;
  readonly authEpoch?: number;
  readonly viewerId?: string;
  readonly suspended?: boolean;
  readonly authorizedLineage?: AgentLoadLineage;
  readonly summary?: AgentSummary;
  readonly onHierarchyChange?: () => void;
  readonly onDeleted?: () => void;
}
