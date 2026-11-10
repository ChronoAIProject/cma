import type { AgentTranscript } from '@chronoai/cma-sdk';
import { object } from './environment';

export function completedFirstAnswer(transcript: AgentTranscript, agentId: string) {
  return (
    transcript.agent_id === agentId &&
    transcript.turns.length === 1 &&
    transcript.turns[0]?.outcome.type === 'completed' &&
    transcript.turns[0].messages.some(
      (message) => message.role === 'assistant' && message.content.trim().length > 0,
    )
  );
}

export function assertVisitorSandbox(value: unknown, sandboxId: string, subject: string) {
  const sandbox = object(value, 'journey Sandbox');
  if (!subject || !sandboxId || sandbox.id !== sandboxId || sandbox.owner !== subject)
    throw new Error('Invalid E2E journey Sandbox: expected the authenticated visitor owner');
}
