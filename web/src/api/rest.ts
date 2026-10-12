import { ProtocolError, type Cma } from '@chronoai/cma-sdk';
import { AgentLifecycleCreateError } from './client';
import type {
  AgentLifecycleConnectionRequired,
  AgentLifecycleCreateInput,
  CmaWebClient,
  ReadOptions,
} from './client';
import type { ConnectionTarget } from '../connections/client';
import type { ProfileReference } from './types';
import { agentDetail, sandboxDetail, sandboxSummary, workspaceDetail } from './rest/hierarchy';
import { createAgentTransport, type AgentTransportOptions } from '../agent/transport/client';

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
    agentTransport: transport === undefined ? undefined : createAgentTransport(sdk, transport),
    resourceViews: sdk,
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
        try {
          const value = (await response.json()) as {
            detail?: unknown;
            code?: unknown;
            services?: unknown;
            connection_target?: unknown;
          };
          if (typeof value.detail === 'string') detail = value.detail;
          if (typeof value.code === 'string') {
            code = value.code;
            if (value.detail === undefined) detail = value.code;
          }
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
        throw new AgentLifecycleCreateError(detail, response.status, code, connection);
      }
      const value = (await response.json()) as { id?: unknown; status?: unknown };
      if (typeof value.id !== 'string' || value.id.length === 0)
        throw new Error('Agent Manager returned an invalid Agent identity.');
      return {
        id: value.id,
        status: typeof value.status === 'string' ? value.status : undefined,
      };
    },
    botViews: sdk,
    listSandboxes: async (options) => {
      const page = await sdk.sandboxes.list(options);
      return { items: page.items.map(sandboxSummary), next: page.next };
    },
    readSandbox: async (id, options) => sandboxDetail(await sdk.sandboxes.read(id, options)),
    readWorkspace: async (id, options) => workspaceDetail(await sdk.workspaces.read(id, options)),
    readAgent: async (id, options) => agentDetail(await sdk.agents.read(id, options)),
    readAgentResources: (id, options) => sdk.agents.resources(id, options),
    readAgentSource: (id, options) => sdk.agents.sourceAccessible(id, options),
    readAgentProvenance: (id, options) => sdk.agents.provenance(id, options),
    readSandboxProfileCapacity: async (reference, options) => {
      const content = await pinnedSandboxProfile(reference, options);
      const cpuMillis = content.cpu_millis;
      const memoryBytes = content.memory_mib * 1024 ** 2;
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
      const page = await sdk.agents.events(id, { after: options?.cursor, signal: options?.signal });
      return {
        items: page.items.map((event) => ({
          seq: event.seq,
          appendedAt: event.appended_at,
          kind: event.kind,
        })),
        next: page.next,
      };
    },
    readAgentTranscript: async (id, options) => {
      const value = await sdk.agents.transcript(id, options);
      return {
        agentId: value.agent_id,
        throughSeq: value.through_seq,
        turns: value.turns.map((turn) => ({
          turnSequence: turn.turn_sequence,
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
    readAgentUsage: async (id, options) => {
      const value = await sdk.agents.usage(id, options);
      return {
        agentId: value.agent_id,
        throughSeq: value.through_seq,
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
    deleteSandbox: async (id, options) => {
      await sdk.sandboxes.delete(id, options);
    },
    removeWorkspace: async (id, options) => {
      await sdk.workspaces.delete(id, options);
    },
    interruptAgent: (id, options) => sdk.agents.interrupt(id, options),
    stopAgent: (id, options) => sdk.agents.stop(id, {}, options),
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
      candidate.type === 'launch') &&
    typeof candidate.id === 'string' &&
    candidate.id.length > 0
  )
    return true;
  if (candidate.type !== 'sandbox_profile' || typeof candidate.profile !== 'object') return false;
  const profile = candidate.profile as { id?: unknown; revision?: unknown };
  return (
    typeof profile.id === 'string' &&
    profile.id.length > 0 &&
    typeof profile.revision === 'number' &&
    Number.isSafeInteger(profile.revision) &&
    profile.revision > 0
  );
}
