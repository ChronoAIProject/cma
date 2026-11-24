import { TransportError, type V2AgentEventItem, type V2AgentEventPage } from '@cma/client';
import type { CmaWebClient } from '../../api/client';
import type { AgentTranscript } from '../../api/types';
import { conversationFromTranscript } from '../conversation';
import { mergeTranscriptSnapshot } from '../chronology';
import { transientReadFailure, type AgentLoadLineage, type ReadyAgent } from '../loadAgent';
import type { ConversationState } from '../model';
import { sameApprovalProjection, withApprovalProjection } from './externalApprovalProjection';
import { foldV2Item } from './projection';
import type { AgentV2Session } from './client';
import { conversationFromHead } from './liveHead';
import { createIdentityHandoff, IDENTITY_HANDOFF_ENABLED } from './identityGate';
import { createProjectionPublication } from './publication';
import { readAgentProjections } from './projections';
import type { AuthorizedOpeningReads } from './openingReads';
import { IdentityChangedError } from './identity';
import { ApprovalBaselineRequired } from './approvalBaseline';
import { OPENING_EVENT_PAGE_LIMIT } from './openingReplay';

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
  opening?: () => Promise<AuthorizedOpeningReads>,
): Promise<ReadyAgent & { readonly durableEvents: readonly [] }> {
  await v2.authorizeLoad(signal);
  // The same fresh check can publish observed preparation and admit this batch.
  // This synchronous callback cannot supply or retain authorization itself.
  onReadAuthorized?.();
  signal.throwIfAborted();
  v2.journal.read();
  const prepared = opening === undefined ? undefined : await opening();
  signal.throwIfAborted();
  prepared?.assertCurrent();
  if (prepared !== undefined && prepared.owner !== v2.owner) throw new IdentityChangedError();
  const reads = prepared?.reads ?? readAgentProjections(client, agentId, signal, lineage);
  const readTranscript = async () => {
    try {
      return await reads.transcript;
    } catch (error) {
      if (signal.aborted || !allowIncomplete || !transientReadFailure(error)) throw error;
      return undefined;
    }
  };
  let pageHandoff: (() => boolean) | undefined;
  const readPage = async (
    pageCursor?: string,
    preparedPage?: Promise<V2AgentEventPage | undefined>,
  ) => {
    try {
      // Only the immediately preceding post-page proof can admit the next
      // page. Publication callbacks and expired handoffs require a fresh read.
      if (pageCursor !== undefined && !pageHandoff?.()) await v2.authorize(signal);
      pageHandoff = undefined;
      signal.throwIfAborted();
      v2.journal.read();
      if (preparedPage !== undefined) return await preparedPage;
      return await v2.events.snapshot(agentId, {
        limit: OPENING_EVENT_PAGE_LIMIT,
        pageCursor,
        signal,
        externalApprovals: true,
        messageBindings: true,
        toolOutput: true,
        retainedOutput: true,
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
  const hierarchy = reads.hierarchy;
  let completedFirstPage: V2AgentEventPage | undefined;
  let observedHead: ConversationState | undefined;
  let usageComplete = false;
  const usageRead = reads.usage.then((usage) => {
    usageComplete = true;
    return usage;
  });
  const preparedHistoryComplete = () => {
    const transcript = reads.completedTranscript();
    return (
      prepared?.replay !== undefined &&
      usageComplete &&
      transcript !== undefined &&
      completedFirstPage?.next_page_cursor === null &&
      observedHead?.agentIncarnation === completedFirstPage.incarnation &&
      Math.max(transcript.throughSeq, observedHead.cursor) <=
        Number(completedFirstPage.through_cursor.split('.').at(-1))
    );
  };
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
          // A complete prepared history goes straight to the final fresh
          // check. Avoid mounting a loading notice only to fade it out again.
          if (preparedHistoryComplete()) return;
          onInputReady({
            ...metadata,
            v2,
            v2Items: [],
            conversation: conversationFromTranscript({ agentId, turns: [], throughSeq: 0 }),
            historyLoading: true,
          });
        });
  const publishTranscript = async (transcript: AgentTranscript | undefined) => {
    const preview = transcript === undefined ? undefined : conversationFromTranscript(transcript);
    if (preview !== undefined && onInputReady !== undefined)
      await publish((metadata) => {
        if (preparedHistoryComplete()) return;
        onInputReady({
          ...metadata,
          v2,
          v2Items: [],
          conversation: preview,
          historyLoading: true,
        });
      });
    return preview;
  };
  // A completed route read can join metadata's fresh publication check now.
  // Pending data still waits for completion and can only join a later batch.
  const completedTranscript = reads.completedTranscript();
  const transcriptPublished =
    completedTranscript === undefined
      ? readTranscript().then(publishTranscript)
      : publishTranscript(completedTranscript);
  const projections = Promise.all([hierarchy, transcriptPublished]).then(([metadata, preview]) => ({
    ...metadata,
    preview,
  }));
  const firstPageRead = readPage(undefined, prepared?.replay?.firstPage).then((page) => {
    completedFirstPage = page;
    return page;
  });
  const liveHead = (async () => {
    if (onInputReady === undefined) return undefined;
    try {
      const head =
        prepared?.replay === undefined
          ? await v2.events.head(agentId, { signal, externalApprovals: true })
          : await prepared.replay.head;
      if (head === undefined) return undefined;
      const conversation = conversationFromHead(head);
      observedHead = conversation;
      // A head publication would queue behind this metadata publication anyway.
      // If the complete fixed prefix has arrived meanwhile, the final fresh
      // check can publish it directly. A pending/partial/older prefix still
      // needs its independent head so live controls never wait for history.
      await hierarchyPublished;
      if (
        completedFirstPage === undefined ||
        completedFirstPage.next_page_cursor !== null ||
        conversation.cursor > Number(completedFirstPage.through_cursor.split('.').at(-1))
      )
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
    usageRead,
    firstPageRead,
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
    if (page.externalApprovalProjection) {
      if (
        [conversation, head].some(
          (prior) =>
            prior?.externalApprovalProjection &&
            !sameApprovalProjection(
              prior.externalApprovalProjection,
              page!.externalApprovalProjection!,
            ),
        )
      ) {
        v2.invalidateApprovalProjection();
        throw new ApprovalBaselineRequired('policy');
      }
      conversation = withApprovalProjection(conversation, page.externalApprovalProjection);
    }
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
