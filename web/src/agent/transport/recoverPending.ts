import type { AgentV2Session } from './client';
import { recoverV2Control } from './controls';
import { recoverV2Interruption } from './interruption';

export async function recoverPendingV2Action(session: AgentV2Session, signal: AbortSignal) {
  if (session.journal.readControl()?.commandId === null) await recoverV2Control(session, signal);
  if (session.journal.interruption.read()?.receipt === null)
    await recoverV2Interruption(session, signal);
  if (session.journal.read()?.state === 'pending') return session.recover(() => {}, signal);
}
