import type { CmaWebClient } from '../api/client';
import type { ResourceViewsClient } from './client';
import { conversationFromTranscript, foldAgentEvent } from '../agent/conversation';
import { mergeTranscriptSnapshot } from '../agent/chronology';
import type { EffectiveAccess } from '@chronoai/cma-sdk';

export class SharedAgentAccessChangedError extends Error {
  constructor() {
    super('Agent access changed. Refresh to check current access.');
  }
}

function accessDeadline(started: number, revalidateAt: string | null | undefined) {
  return Math.min(started + 55_000, revalidateAt == null ? Infinity : Date.parse(revalidateAt));
}

function assertDeadline(deadline: number, signal: AbortSignal) {
  signal.throwIfAborted();
  if (!Number.isFinite(deadline) || deadline <= Date.now())
    throw new SharedAgentAccessChangedError();
}

export function sharedAgentAuthority(access: EffectiveAccess) {
  return JSON.stringify([
    access.policy_revision,
    [...access.capabilities].sort(),
    access.shared_by?.subject,
  ]);
}

/** Access gates interaction; downloading history must not hold up this decision. */
export async function loadSharedAgentAccess(
  client: ResourceViewsClient,
  agentId: string,
  signal: AbortSignal,
) {
  const started = Date.now();
  const access = await client.resourceAccess.effective('agent', agentId, { signal });
  if (!access.capabilities.includes('view_history'))
    throw new Error('Agent history is unavailable.');
  const deadline = accessDeadline(started, access.revalidate_at);
  assertDeadline(deadline, signal);
  return {
    value: { agentId, access, deadline },
    revalidateAt: new Date(deadline).toISOString(),
  };
}

/** A bounded live baseline has the same shared authority and deadline as history. */
export async function loadSharedAgentHead(
  client: ResourceViewsClient,
  authorization: Awaited<ReturnType<typeof loadSharedAgentAccess>>['value'],
  signal: AbortSignal,
) {
  if (client.agentEventsV2 === undefined) return undefined;
  assertDeadline(authorization.deadline, signal);
  const head = await client.agentEventsV2.head(authorization.agentId, { signal });
  const current = await client.resourceAccess.effective('agent', authorization.agentId, { signal });
  assertDeadline(
    Math.min(authorization.deadline, accessDeadline(Date.now(), current.revalidate_at)),
    signal,
  );
  if (
    sharedAgentAuthority(current) !== sharedAgentAuthority(authorization.access) ||
    !current.capabilities.includes('view_history')
  )
    throw new SharedAgentAccessChangedError();
  return head;
}

/** Agent-only reads; a narrow grant does not authorize ancestry or profile requests. */
export async function loadSharedAgentTranscript(
  web: CmaWebClient,
  client: ResourceViewsClient,
  authorization: Awaited<ReturnType<typeof loadSharedAgentAccess>>['value'],
  signal: AbortSignal,
) {
  const { agentId, access } = authorization;
  assertDeadline(authorization.deadline, signal);
  const transcript = await web.readAgentTranscript(agentId, { signal });
  if (transcript.agentId !== agentId) throw new Error('Agent history identity changed.');
  const current = await client.resourceAccess.effective('agent', agentId, { signal });
  const deadline = Math.min(
    authorization.deadline,
    accessDeadline(Date.now(), current.revalidate_at),
  );
  assertDeadline(deadline, signal);
  if (
    sharedAgentAuthority(current) !== sharedAgentAuthority(access) ||
    !current.capabilities.includes('view_history')
  )
    throw new SharedAgentAccessChangedError();
  return {
    value: {
      agentId,
      conversation: conversationFromTranscript(transcript),
      access: current,
      deadline,
    },
    revalidateAt: new Date(deadline).toISOString(),
  };
}

export async function loadSharedAgent(
  web: CmaWebClient,
  client: ResourceViewsClient,
  agentId: string,
  signal: AbortSignal,
) {
  const authorization = await loadSharedAgentAccess(client, agentId, signal);
  return loadSharedAgentTranscript(web, client, authorization.value, signal);
}

/** Rebuild rich history offscreen; the already-authorized transcript remains visible. */
export async function hydrateSharedAgent(
  web: CmaWebClient,
  client: ResourceViewsClient,
  snapshot: Awaited<ReturnType<typeof loadSharedAgent>>['value'],
  signal: AbortSignal,
  throughSequence?: number,
) {
  const { agentId, access, deadline } = snapshot;
  let conversation = conversationFromTranscript({ agentId, turns: [], throughSeq: 0 });
  let cursor: number | undefined = 0;
  do {
    assertDeadline(deadline, signal);
    const page = await web.readAgentEvents(agentId, { cursor, signal });
    assertDeadline(deadline, signal);
    conversation = page.items
      .filter((event) => throughSequence === undefined || event.seq <= throughSequence)
      .reduce(foldAgentEvent, conversation);
    if (page.next !== undefined && page.next <= (cursor ?? 0))
      throw new Error('Agent history pagination did not advance.');
    cursor =
      throughSequence !== undefined && page.items.some((event) => event.seq >= throughSequence)
        ? undefined
        : page.next;
  } while (cursor !== undefined);
  conversation = mergeTranscriptSnapshot(
    conversation,
    snapshot.conversation,
    conversation.cursor,
    true,
  );
  const current = await client.resourceAccess.effective('agent', agentId, { signal });
  assertDeadline(Math.min(deadline, accessDeadline(Date.now(), current.revalidate_at)), signal);
  if (
    sharedAgentAuthority(current) !== sharedAgentAuthority(access) ||
    !current.capabilities.includes('view_history')
  )
    throw new SharedAgentAccessChangedError();
  return conversation;
}
