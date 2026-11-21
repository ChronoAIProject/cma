import type { Agent, AgentListOptions, Cma, OperationOptions } from '@cma/client';
import { agentDisplayTitle } from '../agent/presentation';
import { measuredUsageReader } from './metrics/readMeasuredUsage';
import type { ReadMeasuredUsage } from './metrics/useMeasuredUsage';
import type { MetricsClient } from './metrics/model';

/** Account usage deliberately exposes only owner-scoped SDK operations. */
export interface UsageClient {
  readonly readMeasuredUsage: ReadMeasuredUsage;
  readonly usageMetrics: MetricsClient;
  readonly usage: Pick<Cma['usage'], 'events'>;
  readonly agents: {
    list(
      options: AgentListOptions,
    ): Promise<{ readonly items: readonly UsageAgent[]; readonly next?: string | null }>;
    read(id: string, options: OperationOptions): Promise<UsageAgent>;
  };
}

export interface UsageAgent {
  readonly id: string;
  readonly sandboxId: string;
  readonly name: string;
}

function agentName(agent: Pick<Agent, 'id' | 'title' | 'owner_title' | 'runtime_metadata'>) {
  const title = agent.title?.trim();
  const configured =
    agent.owner_title?.trim() ||
    (title !== agent.id && title !== `Agent ${agent.id}` ? title : undefined);
  if (configured) return configured;
  const name = agentDisplayTitle({
    id: agent.id,
    title: agent.title,
    runtimeMetadata: agent.runtime_metadata && {
      workingDirectory: agent.runtime_metadata.working_directory,
      description: agent.runtime_metadata.description,
      descriptionSource: agent.runtime_metadata.description_source,
      descriptionIsSetupPreview: agent.runtime_metadata.description_is_setup_preview,
    },
  });
  return name === 'Untitled task' ? 'Untitled Agent' : name;
}

export function createUsageClient(client: Cma): UsageClient {
  return {
    readMeasuredUsage: measuredUsageReader(client.auth),
    usageMetrics: client.usageMetrics,
    usage: client.usage,
    agents: {
      async list(options) {
        const page = await client.agents.list(options);
        return {
          next: page.next,
          items: page.items.map((agent) => ({
            id: agent.id,
            sandboxId: agent.sandbox_id,
            name: agentName(agent),
          })),
        };
      },
      async read(id, options) {
        const agent = await client.agents.read(id, options);
        return { id: agent.id, sandboxId: agent.sandbox.id, name: agentName(agent) };
      },
    },
  };
}
