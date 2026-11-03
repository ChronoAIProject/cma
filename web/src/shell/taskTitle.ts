import type { AgentSummary } from '../api/types';
import { agentDisplayTitle } from '../agent/presentation';

export function taskTitle(
  agent: Pick<AgentSummary, 'id' | 'title' | 'ownerTitle' | 'runtimeMetadata'>,
): string {
  const title = agentDisplayTitle(agent);
  if (agent.ownerTitle?.trim()) return title;
  return title === 'Untitled agent' || title === agent.id || title === `Agent ${agent.id}`
    ? 'Untitled task'
    : title;
}
