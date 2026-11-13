import { TransportError, type V2AgentEventItem } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../../api/client';
import { conversationFromTranscript } from '../conversation';
import { mergeTranscriptSnapshot } from '../chronology';
import { transientReadFailure, type AgentLoadLineage, type ReadyAgent } from '../loadAgent';
import type { ConversationState } from '../model';
import { foldV2Item } from './projection';
import type { AgentV2Session } from './client';
import { conversationFromHead } from './liveHead';
import { createIdentityHandoff, IDENTITY_HANDOFF_ENABLED } from './identityGate';
import { createProjectionPublication } from './publication';

export async function loadV2Agent(
  client: CmaWebClient,
  agentId: string,
  signal: AbortSignal,
  v2: AgentV2Session,
  allowIncomplete = false,
  onProgress?: (ready: ReadyAgent) => void,
  lineage?: AgentLoadLineage,
  onInputReady?: (ready: ReadyAgent) => void,
  onReadAuthorized?: () => void,
): Promise<ReadyAgent & { readonly durableEvents: readonly [] }> {
  await v2.authorizeLoad(signal);
  // The same fresh check can publish observed preparation and admit this batch.
  // This synchronous callback cannot supply or retain authorization itself.
  onReadAuthorized?.();
  signal.throwIfAborted();
  v2.journal.read();
  const readHierarchy = async () => {
    const detailRead = (async () => client.readAgent(agentId, { signal }))();
    const [detail, preparedWorkspace] = await Promise.all([
      detailRead,
      lineage?.agentId === agentId
        ? (async () => client.readWorkspace(lineage.workspaceId, { signal }))()
        : undefined,
    ]);
    signal.throwIfAborted();
    if (
      detail.id !== agentId ||
      (lineage !== undefined &&
        (lineage.agentId !== agentId ||
          lineage.workspaceId !== detail.workspace.id ||
          lineage.sandboxId !== detail.sandbox.id))
    )
      throw new Error('Agent projections changed identity.');
    const workspace =
      preparedWorkspace ?? (await client.readWorkspace(detail.workspace.id, { signal }));
    signal.throwIfAborted();
    if (workspace.id !== detail.workspace.id || workspace.sandboxId !== detail.sandbox.id)
      throw new Error('Agent projections changed identity.');
    return { detail, workspace };
  };
  const readUsage = async () => {
    const usage = await client.readAgentUsage(agentId, { signal });
    signal.throwIfAborted();
    if (usage.agentId !== agentId) throw new Error('Agent projections changed identity.');
    return usage;
  };
  const readTranscript = async () => {
    try {
      const transcript = await client.readAgentTranscript(agentId, { signal });
      signal.throwIfAborted();
      if (transcript.agentId !== agentId) throw new Error('Agent projections changed identity.');
      return transcript;
    } catch (error) {
      if (signal.aborted || !allowIncomplete || !transientReadFailure(error)) throw error;
      return undefined;
    }
  };
  let pageHandoff: (() => boolean) | undefined;
  const readPage = async (pageCursor?: string) => {
    try {
      // Only the immediately preceding post-page proof can admit the next
      // page. Publication callbacks and expired handoffs require a fresh read.
      if (pageCursor !== undefined && !pageHandoff?.()) await v2.authorize(signal);
      pageHandoff = undefined;
      signal.throwIfAborted();
      v2.journal.read();
      return await v2.events.snapshot(agentId, {
        pageCursor,
        signal,
        messageBindings: true,
        toolOutput: true,
      });
    } catch (error) {
      if (signal.aborted || !allowIncomplete || !transientReadFailure(error)) throw error;
      return undefined;
    }
  };
  // Input authority and the latest transcript do not depend on rich event pages.
  // Observe every parallel failure even if another read or early callback fails.
  // Projections that finish before metadata can share one publication check.
  // A later completion never joins a check that has already started.
  const hierarchy = readHierarchy();
  const publish = createProjectionPublication(
    hierarchy,
    () => v2.authorize(signal),
    () => v2.journal.read(),
    signal,
  );
  const hierarchyPublished =
    onInputReady === undefined
      ? hierarchy
      : publish((metadata) => {
          onInputReady({
            ...metadata,
            v2,
            v2Items: [],
            conversation: conversationFromTranscript({ agentId, turns: [], throughSeq: 0 }),
            historyLoading: true,
          });
        });
  const transcriptPublished = readTranscript().then(async (transcript) => {
    const preview = transcript === undefined ? undefined : conversationFromTranscript(transcript);
    if (preview !== undefined && onInputReady !== undefined)
      await publish((metadata) => {
        onInputReady({
          ...metadata,
          v2,
          v2Items: [],
          conversation: preview,
          historyLoading: true,
        });
      });
    return preview;
  });
  const projections = Promise.all([hierarchy, transcriptPublished]).then(([metadata, preview]) => ({
    ...metadata,
    preview,
  }));
  const liveHead = (async () => {
    if (onInputReady === undefined) return undefined;
    try {
      const head = await v2.events.head(agentId, { signal });
      const conversation = conversationFromHead(head);
      await publish((metadata) => {
        onInputReady({ ...metadata, v2, v2Items: [], conversation, historyLoading: true });
      });
      return conversation;
    } catch (error) {
      if (signal.aborted || !allowIncomplete || !transientReadFailure(error)) throw error;
      return undefined;
    }
  })();
  const [{ detail, workspace, preview: latest }, usage, firstPage, head] = await Promise.all([
    projections,
    readUsage(),
    readPage(),
    liveHead,
    hierarchyPublished,
  ]).finally(() => publish.dispose());
  // Preserve the final fresh check after every early publication callback.
  await v2.authorize(signal);
  pageHandoff = createIdentityHandoff(IDENTITY_HANDOFF_ENABLED);
  const preview = latest === undefined ? undefined : { ...latest, usage };
  let conversation: ConversationState = {
    ...conversationFromTranscript({ agentId, throughSeq: 0, turns: [] }),
    usage,
  };
  const items: V2AgentEventItem[] = [];
  let incarnation: string | undefined;
  let page = firstPage;
  // The two projections are read concurrently. If the transcript observed a
  // later commit, take a new fixed event prefix before displaying it so live
  // replay can never move backwards from that preview. The SDK validates this
  // cursor's format and sequence; it remains opaque to the request machinery.
  if (
    page !== undefined &&
    Math.max(preview?.cursor ?? 0, head?.cursor ?? 0) >
      Number(page.through_cursor.split('.').at(-1))
  ) {
    page = await readPage();
    await v2.authorize(signal);
    pageHandoff = createIdentityHandoff(IDENTITY_HANDOFF_ENABLED);
    if (
      page !== undefined &&
      Math.max(preview?.cursor ?? 0, head?.cursor ?? 0) >
        Number(page.through_cursor.split('.').at(-1))
    ) {
      if (!allowIncomplete)
        throw new TransportError('Agent transcript is ahead of its available event history.');
      page = undefined;
    }
  }
  let previewPublished = onInputReady !== undefined && preview !== undefined;
  while (page !== undefined) {
    if (incarnation !== undefined && incarnation !== page.incarnation)
      throw new Error('Agent snapshot changed conversation identity.');
    incarnation = page.incarnation;
    if (head !== undefined && incarnation !== head.agentIncarnation)
      throw new Error('Agent snapshot changed conversation identity.');
    conversation = { ...conversation, agentIncarnation: incarnation };
    for (const item of page.items) conversation = foldV2Item(conversation, item);
    items.push(...page.items);
    const pageCursor = page.next_page_cursor ?? undefined;
    if (pageCursor === undefined) {
      conversation = { ...conversation, agentCursor: page.through_cursor };
      break;
    }
    // One complete transcript is the visible baseline. Historical event pages
    // rebuild tools and approvals offscreen; replaying each page in the view
    // briefly resurrects old turns and makes the conversation jump backwards.
    if (!previewPublished && preview !== undefined) {
      if (onProgress !== undefined) pageHandoff = undefined;
      onProgress?.({
        detail,
        workspace,
        conversation: preview,
        v2,
        v2Items: [],
        historyLoading: true,
      });
      previewPublished = true;
    }
    page = await readPage(pageCursor);
    await v2.authorize(signal);
    pageHandoff = createIdentityHandoff(IDENTITY_HANDOFF_ENABLED);
  }
  const complete = page !== undefined;
  const visible =
    preview === undefined
      ? complete
        ? conversation
        : { ...conversationFromTranscript({ agentId, throughSeq: 0, turns: [] }), usage }
      : complete
        ? mergeTranscriptSnapshot(conversation, preview, conversation.cursor, true)
        : preview;
  return {
    detail,
    workspace,
    conversation: visible,
    v2,
    v2Items: items,
    durableEvents: [],
    historyLoading: false,
    historyUnavailable: page === undefined,
  };
}
