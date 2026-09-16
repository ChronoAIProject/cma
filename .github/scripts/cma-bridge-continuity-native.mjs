import { createHash } from 'node:crypto';
import { compareSnapshots, digest, eventPrefix } from './cma-bridge-continuity-observer.mjs';
const requireValue = (condition, code) => { if (!condition) throw new Error(code); };
const hash = (value) => createHash('sha256').update(value).digest('hex');

export function nativeIdentity(items, acceptedResumption) {
  const opened = items.filter((event) => event.kind.type === 'conversation_opened');
  let checked = items;
  if (acceptedResumption) {
    const { events, identity } = acceptedResumption;
    requireValue(events?.length > 0 && items.length >= events.length
      && digest(eventPrefix(items.slice(0, events.length))) === digest(events), 'accepted_resumption_event_prefix_changed');
    const prefix = items.slice(0, events.length);
    const openings = prefix.filter(event => event.kind.type === 'conversation_opened').map(event => ({ sequence: event.seq,
      at: event.appended_at, conversationSha256: hash(event.kind.conversation_id), runtimeGeneration: event.kind.runtime_generation }));
    const retirements = prefix.filter(event => event.kind.type === 'runtime_generation_retired')
      .map(event => ({ sequence: event.seq, at: event.appended_at, ...event.kind }));
    requireValue(digest(openings) === digest(identity.recordedOpenings) && digest(retirements) === digest(identity.recordedRetirements)
      && openings.at(-1)?.conversationSha256 === identity.conversationSha256
      && openings.at(-1)?.runtimeGeneration === identity.runtimeGeneration
      && openings.every(value => value.conversationSha256 === identity.conversationSha256), 'accepted_resumption_native_identity_changed');
    checked = items.slice(events.length);
    requireValue(checked.filter(event => event.kind.type === 'conversation_opened').every(event =>
      hash(event.kind.conversation_id) === identity.conversationSha256 && event.kind.runtime_generation === identity.runtimeGeneration),
    'native_conversation_changed');
  }
  requireValue(opened.length > 0 && !checked.some((event) => event.kind.type === 'runtime_generation_retired'), 'runtime_generation_changed');
  const value = opened.at(-1).kind;
  requireValue(typeof value.conversation_id === 'string' && value.conversation_id.length > 0 && value.runtime_generation != null, 'native_identity_required');
  requireValue(opened.every((event) => event.kind.conversation_id === value.conversation_id
    && (acceptedResumption || event.kind.runtime_generation === value.runtime_generation)), 'native_conversation_changed');
  return { conversationSha256: hash(value.conversation_id), runtimeGeneration: value.runtime_generation, openings: opened.length };
}
export const stableNative = (values) => values.map(({ conversationSha256, runtimeGeneration }) => ({ conversationSha256, runtimeGeneration }));

export function compareRetained(before, after, allowActiveTurn = false) {
  const continuity = compareSnapshots(before.snapshot, after.snapshot);
  requireValue(continuity.status === 'passed', 'retained_runtime_or_event_prefix_changed');
  requireValue(digest(stableNative(before.agents.map((value) => value.native)))
    === digest(stableNative(after.agents.map((value) => value.native))), 'retained_native_identity_changed');
  for (const [index, prior] of before.agents.entries()) {
    const current = after.agents[index];
    requireValue(current.agent.worktree.id === prior.agent.worktree.id, 'retained_worktree_changed');
    for (const [turnIndex, previous] of prior.transcript.turns.entries()) {
      const next = current.transcript.turns[turnIndex];
      if (allowActiveTurn && previous.outcome.type === 'active') {
        requireValue(next && ['turn_sequence', 'started_at', 'owner_message'].every((field) => next[field] === previous[field])
          && digest(next.messages.slice(0, previous.messages.length)) === digest(previous.messages), 'retained_active_turn_prefix_changed');
      } else requireValue(digest(next) === digest(previous), 'retained_transcript_prefix_changed');
    }
  }
  return continuity;
}
