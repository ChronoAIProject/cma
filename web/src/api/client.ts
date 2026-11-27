import type {
  AgentDetail,
  AgentEvent,
  AgentLiveEvent,
  AgentRunRequest,
  AgentTranscript,
  AgentUsage,
  ConfiguredSandboxCapacity,
  CreateAgentInput,
  CreateInlineWorkspaceInput,
  CreateSandboxInput,
  CreateWorkspaceInput,
  ProfileReference,
  ResourcePage,
  SandboxDetail,
  SandboxEvent,
  SandboxSummary,
  SandboxRuntimeUpdate,
  SandboxProfileSettings,
  WorkspaceDetail,
} from './types';
import type {
  Cma,
  AccessibleAgentSource,
  AgentAttachment,
  AgentProvenance,
  AgentResources,
} from '@cma/client';
import type { ResourceViewsClient } from '../resources/client';
import type { BotClient } from '../bots/model';
import type { createAgentTransport } from '../agent/transport/client';
import type { ConnectionTarget } from '../connections/client';
import type {
  RepositoryAccessRefusal,
  RepositoryScopeRepair,
} from '../api-client/agents/lifecycle';
import type { AgentFilesClient } from './agentFiles';
import type { AgentBrowserClient, AgentBrowserControlClient } from '@cma/client';
import type { DocumentsClient } from './documents';

export interface ReadOptions {
  readonly signal?: AbortSignal;
}

export interface PageOptions<Cursor = string> extends ReadOptions {
  readonly cursor?: Cursor;
}

export interface CommandOptions extends ReadOptions {
  readonly idempotencyKey: string;
}

export interface AgentStreamOptions extends ReadOptions {
  readonly idempotencyKey?: string;
  readonly after?: number;
  readonly onEvent: (event: AgentLiveEvent) => void;
}

export interface AgentLifecycleCreateInput {
  readonly name: string;
  readonly title_mode?: 'automatic' | 'named';
  readonly initial_message?: string;
  readonly template?: import('../api-client/agents/task-template').TaskTemplate;
  readonly desktop?: boolean;
  readonly machine_included?: boolean;
  readonly workspace_id?: string;
  readonly sandbox_id?: string;
  readonly image?: { readonly id: string; readonly version?: string };
  readonly repository?: { readonly url: string; readonly ref?: string };
  readonly profile?: {
    readonly model?: string;
    readonly reasoning_effort?: string;
    readonly permission_mode?: 'guarded' | 'autonomous';
    readonly system_prompt?: string;
    readonly skills?: readonly { readonly id: string; readonly version?: string }[];
    readonly permissions?: { readonly services: readonly string[] };
  };
}

export interface AgentSkillState {
  readonly id: string;
  readonly name?: string;
  readonly version: string;
  readonly source: 'ornn';
  readonly status: 'installed' | 'pending' | 'failed';
  readonly error?: string;
}
export interface AgentSkillsView {
  readonly can_install: boolean;
  readonly unavailable_reason?: string;
  readonly items: readonly AgentSkillState[];
  readonly pending: boolean;
}

export interface AgentModelOption {
  readonly id: string;
  readonly label: string;
  readonly available: boolean;
}

export interface AgentPreferencesView {
  readonly title?: string;
  readonly system_prompt: string;
  readonly prompt_pending: boolean;
  readonly prompt_supported: boolean;
  readonly revision: number;
}
export interface UpdateAgentPreferences {
  readonly expected_revision: number;
  readonly title?: string;
  readonly system_prompt?: string;
}

export interface AgentSettingsView {
  readonly pending?: boolean;
  readonly model: string;
  readonly reasoning_effort: 'minimal' | 'low' | 'medium' | 'high' | 'x_high';
  readonly permission_mode: 'guarded' | 'autonomous';
}

export type MessageVote = 'up' | 'down' | null;
export interface MessageFeedback {
  readonly message_id: string;
  readonly vote: MessageVote;
}

export interface AgentLifecycleCreateReceipt {
  readonly id: string;
  readonly status?: string;
}

export interface AgentLifecycleOperationEvent {
  readonly sequence: number;
  readonly phase: string;
  readonly status: string;
  readonly message: string;
  readonly occurred_at: string;
}

export interface AgentLifecycleOperationView {
  readonly id: string;
  readonly accepted_at?: string;
  readonly status: string;
  readonly phase: string;
  readonly can_cancel?: boolean;
  readonly agent_id?: string;
  readonly error?: string;
  readonly failure?: import('@cma/client').AgentLifecycleFailure;
  readonly events: readonly AgentLifecycleOperationEvent[];
  readonly connection?: {
    readonly services: readonly string[];
    readonly target: ConnectionTarget;
    readonly repository_access?: RepositoryAccessRefusal;
    readonly repository_scope_repair?: RepositoryScopeRepair;
  };
}

export interface AgentLifecycleConnectionRequired {
  readonly code: 'connection_required';
  readonly services: readonly string[];
  readonly target: ConnectionTarget;
}

export class AgentLifecycleCreateError extends Error {
  readonly status: number;
  readonly code?: string;
  readonly connection?: AgentLifecycleConnectionRequired;
  readonly failure?: import('@cma/client').AgentLifecycleFailure;

  constructor(
    message: string,
    status: number,
    code?: string,
    connection?: AgentLifecycleConnectionRequired,
    failure?: import('@cma/client').AgentLifecycleFailure,
  ) {
    super(message);
    this.name = 'AgentLifecycleCreateError';
    this.status = status;
    this.code = code;
    this.connection = connection;
    this.failure = failure;
  }
}

/** A transport-free view of the public CMA operations consumed by the web app. */
export interface CmaWebClient {
  readonly usageViews?: import('../usage/client').UsageClient;
  /** The verified owner's acceptance of the deployment's published terms. */
  readonly legalConsent?: Pick<Cma['legalConsent'], 'status' | 'accept'>;
  readonly readSuggestedReplies?: (
    agentId: string,
    scope: { readonly turn_sequence: number; readonly answer_sequence: number },
    options?: { readonly signal?: AbortSignal },
  ) => Promise<import('@cma/client').SuggestedReplyReading>;
  readonly deliverables?: Pick<
    Cma['agents'],
    'answerDeliverables' | 'readAnswerDeliverable' | 'downloadAnswerDeliverable'
  > &
    Partial<Pick<Cma['agents'], 'confirmTaskDelivery'>>;
  readonly readFeedbackDetails?: (
    agentId: string,
    messageId: string,
    options?: ReadOptions,
  ) => Promise<import('./feedbackTypes').FeedbackDetails>;
  readonly setFeedbackReasons?: (
    agentId: string,
    messageId: string,
    body: import('./feedbackTypes').FeedbackReasonRequest,
    options?: ReadOptions,
  ) => Promise<import('./feedbackTypes').FeedbackDetails>;
  readonly documents?: DocumentsClient;
  readonly files?: AgentFilesClient;
  readonly browser?: AgentBrowserClient;
  readonly browserControl?: AgentBrowserControlClient;
  readonly readMessageFeedback?: (
    agentId: string,
    options?: ReadOptions,
  ) => Promise<readonly MessageFeedback[]>;
  readonly setMessageFeedback?: (
    agentId: string,
    messageId: string,
    vote: MessageVote,
    options?: ReadOptions,
  ) => Promise<MessageFeedback>;
  readonly updateSandboxRuntime?: (
    id: string,
    targetVersion: string,
    options: CommandOptions,
  ) => Promise<{ readonly runtimeUpdate: SandboxRuntimeUpdate; readonly revision: number }>;
  readonly inspectSandboxRecovery?: (
    id: string,
    options?: ReadOptions,
  ) => Promise<import('@cma/client').RetainedRecoveryAssessment>;
  readonly recoverSandboxRetained?: (
    id: string,
    expectedRevision: number,
    options: CommandOptions,
  ) => Promise<import('@cma/client').RetainedReopenReceipt>;
  readonly uploadAgentAttachment?: (
    id: string,
    file: File,
    options: CommandOptions,
  ) => Promise<AgentAttachment>;
  readonly agentTransport?: ReturnType<typeof createAgentTransport>;
  readonly resourceViews?: ResourceViewsClient;
  /** New Agent Manager lifecycle facade; absent in compatibility-only clients. */
  readonly readTaskCreationSupport?: (options?: ReadOptions) => Promise<boolean>;
  readonly readTaskTitleModeSupport?: (options?: ReadOptions) => Promise<boolean>;
  readonly taskExperience?: import('@cma/client').TaskExperienceClient;
  readonly readTaskTemplateSupport?: (options?: ReadOptions) => Promise<boolean>;
  readonly readTaskTemplateV2Support?: (options?: ReadOptions) => Promise<boolean>;
  readonly readTaskRepositorySupport?: (options?: ReadOptions) => Promise<boolean>;
  readonly createAgentLifecycle?: (
    input: AgentLifecycleCreateInput,
    options: CommandOptions,
  ) => Promise<AgentLifecycleCreateReceipt>;
  readonly readAgentLifecycle?: (
    operationId: string,
    options?: ReadOptions,
  ) => Promise<AgentLifecycleOperationView>;
  readonly cancelAgentLifecycle?: (
    operationId: string,
    options?: ReadOptions,
  ) => Promise<AgentLifecycleOperationView>;
  readonly readAgentSkills?: (id: string, options?: ReadOptions) => Promise<AgentSkillsView>;
  readonly installAgentSkill?: (
    id: string,
    skill: { readonly id: string; readonly version: string },
    options: CommandOptions,
  ) => Promise<AgentSkillsView>;
  readonly readAgentModels?: (
    id: string,
    options?: ReadOptions,
  ) => Promise<readonly AgentModelOption[]>;
  readonly readAgentPreferences?: (
    id: string,
    options?: ReadOptions,
  ) => Promise<AgentPreferencesView>;
  readonly updateAgentPreferences?: (
    id: string,
    preferences: UpdateAgentPreferences,
    options: CommandOptions,
  ) => Promise<AgentPreferencesView>;
  readonly readAgentSettings?: (id: string, options?: ReadOptions) => Promise<AgentSettingsView>;
  readonly updateAgentSettings?: (
    id: string,
    settings: AgentSettingsView,
    options: CommandOptions,
  ) => Promise<AgentSettingsView>;
  readonly grantSandboxServiceAccess?: (
    id: string,
    services: readonly string[],
    options: CommandOptions,
  ) => Promise<void>;
  readonly botViews?: BotClient;
  readonly readSandboxCapacity?: (
    options?: ReadOptions,
  ) => Promise<import('@cma/client').SandboxCapacity>;
  listSandboxes(
    options?: PageOptions & { readonly excludeRetired?: boolean },
  ): Promise<ResourcePage<SandboxSummary>>;
  readSandbox(id: string, options?: ReadOptions): Promise<SandboxDetail>;
  readWorkspace(id: string, options?: ReadOptions): Promise<WorkspaceDetail>;
  readAgent(id: string, options?: ReadOptions): Promise<AgentDetail>;
  readAgentEvents(
    id: string,
    options?: PageOptions<number> & { readonly cache?: 'no-store' },
  ): Promise<ResourcePage<AgentEvent, number>>;
  readonly readRetainedOutput?: (
    id: string,
    options?: import('@cma/client').RetainedOutputOptions,
  ) => Promise<import('@cma/client').RetainedOutputPage>;
  readonly readAgentReplyPreview?: (
    id: string,
    options?: ReadOptions,
  ) => Promise<import('./replyPreview').ReplyPreview>;
  readonly readAgentOriginalRequest?: (
    id: string,
    options?: ReadOptions,
  ) => Promise<import('./originalRequest').OriginalRequestRead>;
  readAgentTranscript(
    id: string,
    options?: ReadOptions & {
      readonly maxPages?: number;
      readonly maxBytes?: number;
      readonly cache?: 'no-store';
    },
  ): Promise<AgentTranscript>;
  exportAgentHistory(id: string, options?: ReadOptions): Promise<Blob>;
  readAgentUsage(id: string, options?: ReadOptions): Promise<AgentUsage>;
  readAgentResources(id: string, options?: ReadOptions): Promise<AgentResources>;
  readAgentSource(id: string, options?: ReadOptions): Promise<AccessibleAgentSource>;
  readAgentProvenance(id: string, options?: ReadOptions): Promise<AgentProvenance>;
  readSandboxAllocation(
    sandboxId: string,
    options?: ReadOptions,
  ): Promise<ConfiguredSandboxCapacity | undefined>;
  readSandboxProfileSettings(
    reference: ProfileReference,
    options?: ReadOptions,
  ): Promise<SandboxProfileSettings>;
  readSandboxEvents(id: string, options?: PageOptions<number>): Promise<readonly SandboxEvent[]>;
  replayAgent(threadId: string, options: AgentStreamOptions): Promise<void>;
  runAgent(request: AgentRunRequest, options: AgentStreamOptions): Promise<void>;
  createSandbox(input: CreateSandboxInput, options: CommandOptions): Promise<SandboxDetail>;
  createWorkspace(
    sandboxId: string,
    input: CreateWorkspaceInput,
    options: CommandOptions,
  ): Promise<WorkspaceDetail>;
  createInlineWorkspace(
    sandboxId: string,
    input: CreateInlineWorkspaceInput,
    options: CommandOptions,
  ): Promise<WorkspaceDetail>;
  createAgent(
    workspaceId: string,
    input: CreateAgentInput,
    options: CommandOptions,
  ): Promise<AgentDetail>;
  pauseSandbox(id: string, options: CommandOptions): Promise<void>;
  reviveSandbox(id: string, options: CommandOptions): Promise<void>;
  setSandboxAlwaysOn(
    id: string,
    alwaysOn: boolean,
    expectedRevision: number,
    options: CommandOptions,
  ): Promise<SandboxDetail>;
  deleteSandbox(id: string, options: CommandOptions): Promise<void>;
  removeWorkspace(id: string, options: CommandOptions): Promise<void>;
  interruptAgent(id: string, options: CommandOptions): Promise<void>;
  stopAgent(id: string, options: CommandOptions): Promise<void>;
  restartAgent?(id: string, options: CommandOptions): Promise<void>;
  deleteAgent(id: string, options: CommandOptions): Promise<void>;
}

export class ClientUnavailableError extends Error {
  constructor(message = 'The CMA web client is not available in this build.') {
    super(message);
    this.name = 'ClientUnavailableError';
  }
}

function unavailable<T>(): Promise<T> {
  return Promise.reject(new ClientUnavailableError());
}

/** Explicit fallback for unbound tests and unavailable public capabilities. */
export const unavailableClient: CmaWebClient = {
  listSandboxes: async () => ({ items: [] }),
  readSandbox: unavailable,
  readWorkspace: unavailable,
  readAgent: unavailable,
  readAgentEvents: unavailable,
  readAgentTranscript: unavailable,
  exportAgentHistory: unavailable,
  readAgentUsage: unavailable,
  readAgentResources: unavailable,
  readAgentSource: unavailable,
  readAgentProvenance: unavailable,
  readSandboxAllocation: unavailable,
  readSandboxProfileSettings: unavailable,
  readSandboxEvents: unavailable,
  replayAgent: unavailable,
  runAgent: unavailable,
  createSandbox: unavailable,
  createWorkspace: unavailable,
  createInlineWorkspace: unavailable,
  createAgent: unavailable,
  pauseSandbox: unavailable,
  reviveSandbox: unavailable,
  setSandboxAlwaysOn: unavailable,
  deleteSandbox: unavailable,
  removeWorkspace: unavailable,
  interruptAgent: unavailable,
  stopAgent: unavailable,
  deleteAgent: unavailable,
};

type KeyFactory = () => string;

function randomKey(): string {
  if (typeof crypto.randomUUID === 'function') return crypto.randomUUID();
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (value) => value.toString(16).padStart(2, '0')).join('');
}

/** Holds one key per logical command until its outcome is known. */
export class CommandKeys {
  readonly #keys = new Map<string, string>();

  constructor(private readonly makeKey: KeyFactory = randomKey) {}

  for(command: string): string {
    const retained = this.#keys.get(command);
    if (retained !== undefined) return retained;
    const key = this.makeKey();
    this.#keys.set(command, key);
    return key;
  }

  settle(command: string): void {
    this.#keys.delete(command);
  }
}
