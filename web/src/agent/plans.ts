import type { ConversationState, PlanEntry, PlanSnapshot } from './model';
import { object, text } from './conversationValues';

export function reportedPlanProgress(plan: PlanSnapshot) {
  const completed = plan.entries.filter((entry) => entry.status === 'completed').length;
  const currentIndex = plan.incomplete
    ? -1
    : plan.entries.findIndex((entry) => entry.status === 'in_progress');
  const multiple = plan.entries.filter((entry) => entry.status === 'in_progress').length > 1;
  const known =
    !plan.incomplete &&
    !multiple &&
    plan.entries.every((entry) => ['completed', 'in_progress', 'pending'].includes(entry.status));
  return {
    completed,
    currentIndex,
    multiple,
    known,
    summary: plan.incomplete
      ? 'Some plan steps unavailable'
      : known
        ? `Completed ${completed}/${plan.entries.length} steps`
        : `${completed} of ${plan.entries.length} steps reported complete`,
  };
}

export function planAnchor(plan: PlanSnapshot) {
  return `reported-${plan.id}`;
}

/** Entries that are not a titled step with a status are dropped rather than guessed at. */
function planEntries(value: unknown): readonly PlanEntry[] | undefined {
  if (!Array.isArray(value)) return undefined;
  return value.flatMap((entry) => {
    const item = object(entry);
    const title = text(item?.title);
    const status = text(item?.status);
    return title === undefined ||
      title.trim() === '' ||
      status === undefined ||
      status.trim() === ''
      ? []
      : [{ title, status }];
  });
}

/**
 * Replace a turn's plan with its latest report, keyed by turn so the durable
 * history and a live stream restating the same plan update one card rather
 * than adding a second. An empty report clears the card.
 */
export function setPlan(
  state: ConversationState,
  runId: string | undefined,
  value: unknown,
  reportedAt?: string,
): ConversationState {
  const entries = planEntries(value);
  if (entries === undefined) return state;
  const id = `plan:${runId ?? 'unattributed'}`;
  const current = state.plans.find((plan) => plan.id === id);
  const incomplete = entries.length !== (value as unknown[]).length || undefined;
  if (
    current?.incomplete === incomplete &&
    current?.entries.length === entries.length &&
    current.entries.every(
      (entry, index) =>
        entry.title === entries[index]?.title && entry.status === entries[index]?.status,
    )
  )
    return state;
  const plan = {
    ...current,
    id,
    runId,
    entries,
    incomplete,
    progressAt: reportedAt ?? current?.progressAt,
  };
  return {
    ...state,
    plans:
      current === undefined
        ? [...state.plans, plan]
        : state.plans.map((item) => (item.id === id ? plan : item)),
  };
}
