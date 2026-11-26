import type { V2AgentStreamFrame } from '@cma/client';
import { HydrationEvents } from '../mergeHydration';
import { mergeTranscriptSnapshot } from '../chronology';
import type { ConversationState } from '../model';
import { foldV2Frame } from './projection';
import { ApprovalBaselineRequired } from './approvalBaseline';

export const EXTERNAL_APPROVAL_BUFFER_LIMIT = 256;

/** Rebase a live tail onto rich history without dropping or replaying new output. */
export class ConversationHydration extends HydrationEvents {
  private frames: V2AgentStreamFrame[] = [];
  private approvalOverflow = false;
  private incarnation?: string;
  private baseline?: ConversationState;
  private transcript?: ConversationState;
  private authoritative?: Pick<ConversationState, 'cursor' | 'activeRunId'>;

  override reset(snapshot?: ConversationState) {
    super.reset(snapshot);
    this.frames = [];
    this.approvalOverflow = false;
    this.incarnation = snapshot?.agentIncarnation;
    this.baseline = undefined;
    this.transcript = undefined;
    this.authoritative =
      snapshot?.agentIncarnation === undefined
        ? undefined
        : { cursor: snapshot.cursor, activeRunId: snapshot.activeRunId };
  }

  applyV2(
    state: ConversationState,
    frame: V2AgentStreamFrame,
    agent: string,
    buffering: boolean,
  ): ConversationState {
    if (this.approvalOverflow) throw new ApprovalBaselineRequired('overflow');
    if (
      buffering &&
      isExternalApproval(frame) &&
      this.frames.filter(isExternalApproval).length >= EXTERNAL_APPROVAL_BUFFER_LIMIT
    ) {
      this.approvalOverflow = true;
      throw new ApprovalBaselineRequired('overflow');
    }
    // HTTP history can overtake an already released SSE fragment. Only an
    // authoritative newer prefix can prove that its old turn is now obsolete;
    // keep the reducer's identity and same-boundary protocol checks intact.
    if (
      (frame.type === 'cma.agent.live' || frame.type === 'cma.agent.live_gap') &&
      frame.data.agent_id === agent &&
      frame.data.incarnation === state.agentIncarnation &&
      this.incarnation === state.agentIncarnation &&
      this.authoritative !== undefined &&
      frame.data.turn_sequence !== null &&
      String(frame.data.turn_sequence) !== this.authoritative.activeRunId &&
      (frame.type === 'cma.agent.live' ? frame.data.after_sequence : frame.data.turn_sequence) <
        this.authoritative.cursor
    )
      return state;
    const next = foldHydratingFrame(state, frame, agent, buffering);
    this.incarnation = next.agentIncarnation;
    if (buffering && frame.type !== 'cma.agent.error') {
      const previous = this.frames.at(-1);
      if (
        frame.type === 'cma.agent.live' &&
        previous?.type === 'cma.agent.live' &&
        previous.data.kind === frame.data.kind &&
        previous.data.turn_sequence === frame.data.turn_sequence &&
        previous.data.after_sequence === frame.data.after_sequence &&
        previous.data.message_id === frame.data.message_id
      )
        this.frames[this.frames.length - 1] = {
          ...frame,
          data: { ...frame.data, delta: previous.data.delta + frame.data.delta },
        };
      else this.frames.push(frame);
    }
    return next;
  }

  override merge(snapshot: ConversationState, agent: string, complete: boolean) {
    if (this.approvalOverflow) throw new ApprovalBaselineRequired('overflow');
    if (!complete) {
      if (snapshot.agentIncarnation === undefined) this.transcript = snapshot;
      else this.baseline = snapshot;
      if (this.baseline !== undefined) {
        snapshot = this.baseline;
        if (this.transcript !== undefined) {
          const before = {
            ...this.transcript,
            cursor: snapshot.cursor,
            messages: this.transcript.messages.filter(
              (message) => (message.sequence ?? 0) <= snapshot.cursor,
            ),
            recovery: this.transcript.recovery.filter(
              (notice) => (notice.afterSequence ?? 0) <= snapshot.cursor,
            ),
          };
          snapshot = mergeTranscriptSnapshot(snapshot, before);
        }
      }
    }
    if (snapshot.agentIncarnation === undefined) return super.merge(snapshot, agent, complete);
    if (this.incarnation !== undefined && snapshot.agentIncarnation !== this.incarnation)
      throw new Error('Agent snapshot changed conversation identity.');
    this.incarnation = snapshot.agentIncarnation;
    if (this.authoritative === undefined || snapshot.cursor >= this.authoritative.cursor)
      this.authoritative = { cursor: snapshot.cursor, activeRunId: snapshot.activeRunId };
    let conversation = snapshot;
    this.frames = this.frames.filter((frame) => {
      if (frame.type === 'cma.agent.event') {
        if (frame.data.sequence <= snapshot.cursor) return false;
      } else if (frame.type === 'cma.agent.live' || frame.type === 'cma.agent.live_gap') {
        // Hydration may already include completion, cancellation, or a later turn.
        if (String(frame.data.turn_sequence) !== conversation.activeRunId) return false;
        if (
          frame.type === 'cma.agent.live' &&
          frame.data.kind === 'text_delta' &&
          snapshot.messages.some(
            (message) =>
              message.role === 'assistant' &&
              !message.liveOnly &&
              message.runId === String(frame.data.turn_sequence) &&
              (message.sequence ?? 0) > frame.data.after_sequence,
          )
        )
          return false;
      }
      conversation = foldHydratingFrame(conversation, frame, agent, !complete);
      return true;
    });
    if (complete) {
      this.frames = [];
      this.baseline = undefined;
      this.transcript = undefined;
    }
    return conversation;
  }
}

/** A head omits older receipts; wait for the fixed prefix before exposing any approval. */
function foldHydratingFrame(
  state: ConversationState,
  frame: V2AgentStreamFrame,
  agent: string,
  incomplete: boolean,
) {
  if (
    incomplete &&
    frame.type === 'cma.agent.event' &&
    (frame.data.kind?.type === 'external_approval_requested' ||
      frame.data.kind?.type === 'external_approval_resolved')
  ) {
    // Keep the original frame buffered for the complete fold. Identity and cursor
    // checks still apply now; no receipt or action is inferred from partial history.
    return foldV2Frame(state, { ...frame, data: { ...frame.data, kind: null } }, agent);
  }
  return foldV2Frame(state, frame, agent);
}

function isExternalApproval(frame: V2AgentStreamFrame) {
  return (
    frame.type === 'cma.agent.event' &&
    (frame.data.kind?.type === 'external_approval_requested' ||
      frame.data.kind?.type === 'external_approval_resolved')
  );
}
