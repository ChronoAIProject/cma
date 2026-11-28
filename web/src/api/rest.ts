import { createUsageClient } from '../usage/client';
import {
  ProtocolError,
  TransportError,
  agentLifecycleFailure,
  repositoryAccessRefusal,
  repositoryScopeRepair,
  type AgentLifecycleFailure,
  type Cma,
} from '@cma/client';
import { AgentLifecycleCreateError } from './client';
import type {
  AgentLifecycleConnectionRequired,
  AgentLifecycleCreateInput,
  AgentLifecycleOperationView,
  CmaWebClient,
  ReadOptions,
} from './client';
import type { ConnectionTarget } from '../connections/client';
import type { CreateInlineWorkspaceInput, ProfileReference } from './types';
import {
  agentDetail,
  runtimeUpdate,
  sandboxDetail,
  sandboxSummary,
  workspaceDetail,
} from './rest/hierarchy';
import { createAgentTransport, type AgentTransportOptions } from '../agent/transport/client';
import { agentSettingsClient } from './rest/agentSettings';
import { agentSkillsClient } from './rest/agentSkills';
import { messageFeedbackClient } from './rest/messageFeedback';
import { agentFilesClient } from './rest/agentFiles';
import { documentsClient } from './rest/documents';
import { computerClient } from './rest/computer';
import { exportRawHistory } from './rest/history';
import { readReplyPreview } from './rest/replyPreview';
import { readOriginalRequest } from './rest/originalRequest';
import { BusinessReadError, readBusinessProblem } from './rest/businessReadProblem';
import { readWorkspaceDetail } from './rest/workspaceRead';

/** The write succeeded; a failed detail read must not release its command key. */
export class CreatedResourceReadError extends Error {
  constructor(
    readonly resource: 'Sandbox' | 'Workspace',
    readonly resourceId: string,
    cause: unknown,
  ) {
    super(
      `${resource} created, but its details could not be loaded. Retry to load the existing ${resource}.`,
      { cause },
    );
    this.name = 'CreatedResourceReadError';
  }
}

async function createdDetail<Value>(
  resource: 'Sandbox' | 'Workspace',
  id: string,
  read: () => Promise<Value>,
): Promise<Value> {
  try {
    return await read();
  } catch (cause) {
    throw new CreatedResourceReadError(resource, id, cause);
  }
}

/** One public SDK transport, with explicit wire-to-view conversion at the boundary. */
export function createRestWebClient(sdk: Cma, transport?: AgentTransportOptions): CmaWebClient {
  const computer = computerClient(sdk);
  const pinnedSandboxProfile = async (reference: ProfileReference, options?: ReadOptions) => {
    const profile = await sdk.sandboxProfiles.readRevision(
      reference.id,
      reference.revision,
      options,
    );
    if (
      profile.profile_id !== reference.id ||
      profile.revision !== reference.revision ||
      (reference.digest !== undefined && profile.digest !== reference.digest)
    )
      throw new ProtocolError('Sandbox Profile identity mismatch');
    return profile.content;
  };
  return {
    usageViews: createUsageClient(sdk),
    legalConsent: sdk.legalConsent,
    deliverables: sdk.agents,
    documents: documentsClient(sdk.documents),
    browser: sdk.agentBrowser,
    browserControl: computer.browserControl,
    taskExperience: sdk.taskExperience,
    files: {
      ...agentFilesClient(),
      environment: computer.environment,
    },
    updateSandboxRuntime: async (id, targetVersion, options) => {
      const result = await sdk.sandboxes.updateRuntime(
        id,
        { target_version: targetVersion },
        options,
      );
      return { runtimeUpdate: runtimeUpdate(result.runtime_update), revision: result.revision };
    },
    inspectSandboxRecovery: (id, options) => sdk.sandboxes.inspectRuntimeRecovery(id, options),
    recoverSandboxRetained: (id, expectedRevision, options) =>
      sdk.sandboxes.recoverRetained(
        id,
        {
          expected_revision: expectedRevision,
          confirm_same_image_recovery: true,
        },
        options,
      ),
    ...agentSettingsClient(),
    ...messageFeedbackClient(),
    readSuggestedReplies: (agentId, scope, options) =>
      sdk.agents.suggestions(agentId, scope, options),
    ...agentSkillsClient(),
    grantSandboxServiceAccess: async (id, services, options) => {
      await sdk.sandboxes.grantServiceAccess(id, { services: [...services] }, options);
    },
    agentTransport: transport === undefined ? undefined : createAgentTransport(sdk, transport),
    resourceViews: sdk,
    readTaskTemplateSupport: async (options) =>
      (await sdk.readOpenApi(options))['x-cma-task-templates-v1'] === true,
    readTaskTemplateV2Support: async (options) =>
      (await sdk.readOpenApi(options))['x-cma-task-templates-v2'] === true,
    readTaskRepositorySupport: async (options) =>
      (await sdk.readOpenApi(options))['x-cma-task-repository'] === true,
    readTaskTitleModeSupport: async (options) =>
      (await sdk.readOpenApi(options))['x-cma-agent-title-mode'] === true,
    readTaskCreationSupport: async (options) => {
      const document = await sdk.readOpenApi(options);
      const components = document.components as
        | {
            schemas?: {
              AgentLifecycleCreateRequest?: {
                properties?: { initial_message?: { type?: unknown } };
              };
            };
          }
        | undefined;
      const type =
        components?.schemas?.AgentLifecycleCreateRequest?.properties?.initial_message?.type;
      return type === 'string' || (Array.isArray(type) && type.includes('string'));
    },
    createAgentLifecycle: async (input: AgentLifecycleCreateInput, options) => {
      const response = await fetch('/api/agent-manager/v1/agents', {
        method: 'POST',
        credentials: 'same-origin',
        cache: 'no-store',
        signal: options.signal,
        headers: {
          'Content-Type': 'application/json',
          'Idempotency-Key': options.idempotencyKey,
        },
        body: JSON.stringify(input),
      });
      if (!response.ok) {
        let detail = `Agent Manager could not create this Agent (${String(response.status)}).`;
        let code: string | undefined;
        let connection: AgentLifecycleConnectionRequired | undefined;
        let failure: AgentLifecycleFailure | undefined;
        try {
          const value = (await response.json()) as {
            detail?: unknown;
            code?: unknown;
            services?: unknown;
            connection_target?: unknown;
            limit?: unknown;
            live?: unknown;
          };
          if (typeof value.detail === 'string') detail = value.detail;
          if (typeof value.code === 'string') {
            code = value.code;
            if (value.detail === undefined) detail = value.code;
          }
          if (response.status === 429 && code === 'sandbox_limit_reached')
            failure = agentLifecycleFailure(value);
          if (
            code === 'connection_required' &&
            Array.isArray(value.services) &&
            value.services.every((service): service is string => typeof service === 'string') &&
            isConnectionTarget(value.connection_target)
          ) {
            connection = {
              code,
              services: value.services,
              target: value.connection_target,
            };
          }
        } catch {
          /* Keep the status-based message. */
        }
        throw new AgentLifecycleCreateError(detail, response.status, code, connection, failure);
      }
      const value = (await response.json()) as { id?: unknown; status?: unknown };
      if (typeof value.id !== 'string' || value.id.length === 0)
        throw new Error('Agent Manager returned an invalid Agent identity.');
      return {
        id: value.id,
        status: typeof value.status === 'string' ? value.status : undefined,
      };
    },
    readAgentLifecycle: async (
      operationId: string,
      options,
    ): Promise<AgentLifecycleOperationView> => {
      const response = await fetch(
        `/api/agent-manager/v1/agent-lifecycle/${encodeURIComponent(operationId)}`,
        {
          credentials: 'same-origin',
          cache: 'no-store',
          signal: options?.signal,
        },
      ).catch((error: unknown) => {
        options?.signal?.throwIfAborted();
        if (error instanceof TypeError)
          throw new TransportError('Agent lifecycle status could not be reached.', {
            cause: error,
          });
        throw error;
      });
      if (!response.ok)
        throw new BusinessReadError(await readBusinessProblem(response, options?.signal));
      const value = (await response.json().catch((error: unknown) => {
        options?.signal?.throwIfAborted();
        if (error instanceof SyntaxError)
          throw new ProtocolError('Agent Manager returned an invalid lifecycle operation.');
        if (error instanceof TypeError)
          throw new TransportError('Agent lifecycle status could not be read.', { cause: error });
        throw error;
      })) as AgentLifecycleOperationView;
      if (
        !value ||
        typeof value !== 'object' ||
        value.id !== operationId ||
        (value.accepted_at !== undefined &&
          (typeof value.accepted_at !== 'string' ||
            !Number.isFinite(Date.parse(value.accepted_at)))) ||
        !['queued', 'running', 'waiting_for_connection', 'succeeded', 'failed'].includes(
          value.status,
        ) ||
        typeof value.phase !== 'string' ||
        !Array.isArray(value.events) ||
        (value.connection !== undefined &&
          (!value.connection ||
            typeof value.connection !== 'object' ||
            !Array.isArray(value.connection.services) ||
            !value.connection.services.every((service) => typeof service === 'string') ||
            !isConnectionTarget(value.connection.target))) ||
        (value.status === 'waiting_for_connection' && value.connection === undefined) ||
        (value.status === 'succeeded' && (typeof value.agent_id !== 'string' || !value.agent_id))
      )
        throw new ProtocolError('Agent Manager returned an invalid lifecycle operation.');
      if (value.connection?.repository_access !== undefined)
        repositoryAccessRefusal(value.connection.repository_access);
      if (value.connection?.repository_scope_repair !== undefined)
        repositoryScopeRepair(value.connection.repository_scope_repair);
      if (value.failure !== undefined) agentLifecycleFailure(value.failure);
      return value;
    },
    uploadAgentAttachment: (id, file, options) => sdk.agentAttachments.upload(id, file, options),
    botViews: sdk,
    readSandboxCapacity: (options) => sdk.sandboxes.capacity(options),
    listSandboxes: async (options) => {
      const page = await sdk.sandboxes.list(options);
      return { items: page.items.map(sandboxSummary), next: page.next };
    },
    readSandbox: async (id, options) => sandboxDetail(await sdk.sandboxes.read(id, options)),
    readWorkspace: (id, options) => readWorkspaceDetail(sdk, id, options),
    readAgent: async (id, options) => agentDetail(await sdk.agents.read(id, options)),
    readAgentResources: (id, options) => sdk.agents.resources(id, options),
    readAgentSource: (id, options) => sdk.agents.sourceAccessible(id, options),
    readAgentProvenance: (id, options) => sdk.agents.provenance(id, options),
    readSandboxAllocation: async (sandboxId, options) => {
      const { allocation } = await sdk.usage.sandbox(sandboxId, options);
      if (!allocation.running) return undefined;
      const cpuMillis = allocation.cpu_millis;
      const memoryBytes = allocation.memory_mib * 1024 ** 2;
      if (
        !Number.isSafeInteger(cpuMillis) ||
        cpuMillis <= 0 ||
        !Number.isSafeInteger(memoryBytes) ||
        memoryBytes <= 0
      )
        throw new ProtocolError('Invalid configured Sandbox capacity');
      return { cpuMillis, memoryBytes };
    },
    readSandboxProfileSettings: async (reference, options) => {
      const content = await pinnedSandboxProfile(reference, options);
      return {
        agentProvider: content.agent_provider,
        modelAccess: content.model_access,
        nyxidServices: content.nyxid_services,
      };
    },
    readAgentEvents: async (id, options) => {
      const page = await sdk.agents.events(id, {
        after: options?.cursor,
        signal: options?.signal,
        ...(options?.cache === undefined ? {} : { cache: options.cache }),
        toolOutput: true,
      });
      return {
        items: page.items.map((event) => ({
          seq: event.seq,
          appendedAt: event.appended_at,
          retainedOutput: event.retained_output,
          kind: event.kind,
        })),
        next: page.next,
      };
    },
    readRetainedOutput: (id, options) => sdk.agents.retainedOutput(id, options),
    readAgentReplyPreview: (id, options) => readReplyPreview(sdk, id, options),
    readAgentOriginalRequest: (id, options) => readOriginalRequest(sdk, id, options),
    readAgentTranscript: async (id, options) => {
      const value = await sdk.agents.transcript(id, options);
      return {
        agentId: value.agent_id,
        throughSeq: value.through_seq,
        turns: value.turns.map((turn) => ({
          turnSequence: turn.turn_sequence,
          retainedOutput: turn.retained_output,
          startedAt: turn.started_at,
          ownerMessage: turn.owner_message,
          agentMessages: turn.agent_messages,
          messages: turn.messages.map((message) => ({
            sequence: message.sequence,
            role: message.role,
            content: message.content,
            appendedAt: message.appended_at,
          })),
          outcome: turn.outcome,
        })),
      };
    },
    exportAgentHistory: (id, options) => exportRawHistory(sdk, id, options),
    readAgentUsage: async (id, options) => {
      const value = await sdk.agents.usage(id, options);
      return {
        agentId: value.agent_id,
        throughSeq: value.through_seq,
        currentTurn:
          value.current_turn === undefined
            ? undefined
            : {
                turnSequence: value.current_turn.turn_sequence,
                inputTokens: value.current_turn.input_tokens,
                outputTokens: value.current_turn.output_tokens,
              },
        inputTokens: value.input_tokens,
        outputTokens: value.output_tokens,
        contextUsed: value.context_used,
        contextWindow: value.context_window,
      };
    },
    readSandboxEvents: async (id, options) => {
      const events = await sdk.sandboxes.events(id, {
        after: options?.cursor,
        signal: options?.signal,
      });
      return events.map((event) => ({
        sandboxId: event.sandbox_id,
        seq: event.seq,
        appendedAt: event.appended_at,
        kind: event.kind,
      }));
    },
    replayAgent: (id, options) => sdk.agui.replay(id, options),
    runAgent: (request, options) => sdk.agui.run(request, options),
    createSandbox: async (input, options) => {
      const created = await sdk.sandboxes.create(
        {
          sandbox_profile: { id: input.sandboxProfile.id, revision: input.sandboxProfile.revision },
          name: input.name,
          trust_confirmed: input.trustConfirmed,
          commands_digest: input.commandsDigest,
        },
        options,
      );
      return createdDetail('Sandbox', created.id, async () =>
        sandboxDetail(await sdk.sandboxes.read(created.id, { signal: options.signal })),
      );
    },
    createWorkspace: async (sandboxId, input, options) => {
      const created = await sdk.workspaces.create(
        sandboxId,
        {
          workspace_profile: {
            id: input.workspaceProfile.id,
            revision: input.workspaceProfile.revision,
          },
          trust_confirmed: input.trustConfirmed,
          commands_digest: input.commandsDigest,
        },
        options,
      );
      return createdDetail('Workspace', created.id, async () =>
        workspaceDetail(await sdk.workspaces.read(created.id, { signal: options.signal })),
      );
    },
    createInlineWorkspace: async (
      sandboxId: string,
      input: CreateInlineWorkspaceInput,
      options,
    ) => {
      const created = await sdk.workspaces.createInline(sandboxId, input, options);
      return createdDetail('Workspace', created.id, async () =>
        workspaceDetail(await sdk.workspaces.read(created.id, { signal: options.signal })),
      );
    },
    createAgent: async (workspaceId, input, options) =>
      agentDetail(
        await sdk.agents.create(
          workspaceId,
          {
            agent_profile: { id: input.agentProfile.id, revision: input.agentProfile.revision },
            title: input.title,
            first_message: input.firstMessage,
          },
          options,
        ),
      ),
    pauseSandbox: async (id, options) => {
      await sdk.sandboxes.pause(id, options);
    },
    reviveSandbox: async (id, options) => {
      await sdk.sandboxes.revive(id, options);
    },
    setSandboxAlwaysOn: async (id, alwaysOn, expectedRevision, options) => {
      await sdk.sandboxes.setAlwaysOn(
        id,
        { always_on: alwaysOn, expected_revision: expectedRevision },
        options,
      );
      return sandboxDetail(await sdk.sandboxes.read(id, { signal: options.signal }));
    },
    deleteSandbox: async (id, options) => {
      await sdk.sandboxes.delete(id, options);
    },
    removeWorkspace: async (id, options) => {
      await sdk.workspaces.delete(id, options);
    },
    interruptAgent: (id, options) => sdk.agents.interrupt(id, options),
    stopAgent: (id, options) => sdk.agents.stop(id, {}, options),
    restartAgent: async (id, options) => {
      await sdk.agents.restart(id, options);
    },
    deleteAgent: (id, options) => sdk.agents.delete(id, options),
  };
}

function isConnectionTarget(value: unknown): value is ConnectionTarget {
  if (typeof value !== 'object' || value === null) return false;
  const candidate = value as { type?: unknown; id?: unknown; profile?: unknown };
  if (
    (candidate.type === 'sandbox' ||
      candidate.type === 'workspace' ||
      candidate.type === 'agent' ||
      candidate.type === 'launch' ||
      candidate.type === 'agent_access_request') &&
    typeof candidate.id === 'string' &&
    candidate.id.length > 0
  )
    return true;
  if (
    candidate.type !== 'sandbox_profile' ||
    typeof candidate.profile !== 'object' ||
    candidate.profile === null
  )
    return false;
  const profile = candidate.profile as { id?: unknown; revision?: unknown };
  return (
    typeof profile.id === 'string' &&
    profile.id.length > 0 &&
    typeof profile.revision === 'number' &&
    Number.isSafeInteger(profile.revision) &&
    profile.revision > 0
  );
}
