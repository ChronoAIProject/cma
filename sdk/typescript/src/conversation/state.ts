import type { AguiEvent } from '../agui/index.js';
import { applyStateDelta } from '../agui/state.js';
import { GAP_VALUE_FRAMES, PATHS, type AguiState } from '../wire.js';
import type { AvailableCommand, ConfigOption, Conversation, Entry, PlanStep } from './model.js';
import { cpuPressureFrom, memoryPressureFrom, resourcesFrom } from './resources.js';

/** Apply the canonical state first, then derive the fields this surface draws. */
export function reduceStateDelta(
  state: Conversation,
  event: Extract<AguiEvent, { type: 'STATE_DELTA' }>,
): Conversation {
  if (event.ephemeral !== true && event.seq <= state.stateDeltaSeq) {
    return state;
  }

  const previous = state.aguiState;
  const aguiState = applyStateDelta(previous, event.patch);
  let next: Conversation = {
    ...state,
    aguiState,
    stateDeltaSeq: event.ephemeral === true ? state.stateDeltaSeq : event.seq,
  };

  next = projectScalarState(next, previous, aguiState, event);
  next = projectGaps(next, previous, aguiState, event);
  next = projectPlaneChanges(next, previous, aguiState, event);
  next = projectContextCompactions(next, previous, aguiState, event);
  return projectRuntimeLosses(next, previous, aguiState, event);
}

function projectScalarState(
  state: Conversation,
  previous: AguiState,
  current: AguiState,
  event: Extract<AguiEvent, { type: 'STATE_DELTA' }>,
): Conversation {
  let next = state;

  // A returning RUN_ERROR disables interaction without rewriting canonical
  // state, so the later durable Ready report can replace an identical value.
  const statusReported = event.patch.some(
    (operation) => operation.op === 'replace' && operation.path === PATHS.status,
  );
  const status = fieldOf(current, 'status');
  const statusFolded = statusReported || fieldChanged(previous, current, 'status');
  const sessionInteractive = status === 'ready' || status === 'running';
  if (statusFolded) {
    next = {
      ...next,
      // Unknown future values stay noninteractive until this SDK learns what
      // they mean. `running` is lifecycle-interactive; steer admission still
      // depends on the granted capability and concurrent-input policy.
      sessionInteractive,
    };
  }

  if (fieldChanged(previous, current, 'resources')) {
    next = { ...next, resources: resourcesFrom(fieldOf(current, 'resources')) };
  }
  if (fieldChanged(previous, current, 'cpuPressure')) {
    next = { ...next, cpuPressure: cpuPressureFrom(fieldOf(current, 'cpuPressure')) };
  }
  if (fieldChanged(previous, current, 'memoryPressure')) {
    next = { ...next, memoryPressure: memoryPressureFrom(fieldOf(current, 'memoryPressure')) };
  }
  if (statusFolded && !sessionInteractive) {
    // A runtime can disappear before it writes the closing edges. Lifecycle
    // state still makes both derived warnings definitively no longer current.
    next = { ...next, cpuPressure: undefined, memoryPressure: undefined };
  }

  const previousUsage = recordOf(fieldOf(previous, 'usage'));
  const currentUsage = recordOf(fieldOf(current, 'usage'));
  const oldInput = previousUsage?.inputTokens;
  const input = currentUsage?.inputTokens;
  if (!jsonEqual(oldInput, input) && typeof input === 'number') {
    next = {
      ...next,
      usage: {
        ...next.usage,
        inputTokens: input,
        tokens: totalOf(input, next.usage?.outputTokens),
      },
    };
  }
  const oldOutput = previousUsage?.outputTokens;
  const output = currentUsage?.outputTokens;
  if (!jsonEqual(oldOutput, output) && typeof output === 'number') {
    next = {
      ...next,
      usage: {
        ...next.usage,
        outputTokens: output,
        tokens: totalOf(next.usage?.inputTokens, output),
      },
    };
  }
  const oldContextUsed = previousUsage?.contextUsed;
  const contextUsed = currentUsage?.contextUsed;
  if (
    !jsonEqual(oldContextUsed, contextUsed) &&
    typeof contextUsed === 'number' &&
    contextUsed > 0
  ) {
    next = { ...next, usage: { ...next.usage, contextUsed } };
  }
  const oldContextWindow = previousUsage?.contextWindow;
  const contextWindow = currentUsage?.contextWindow;
  if (
    !jsonEqual(oldContextWindow, contextWindow) &&
    typeof contextWindow === 'number' &&
    contextWindow > 0
  ) {
    next = { ...next, usage: { ...next.usage, contextWindow } };
  }
  const oldCost = previousUsage?.cost;
  const cost = currentUsage?.cost;
  if (!jsonEqual(oldCost, cost) && typeof cost === 'string') {
    next = { ...next, usage: { ...next.usage, cost } };
  }

  if (fieldChanged(previous, current, 'signIn')) {
    const value = fieldOf(current, 'signIn');
    if (typeof value === 'object') {
      const signIn = recordOf(value);
      const url = typeof signIn?.url === 'string' ? signIn.url : undefined;
      next = {
        ...next,
        signIn:
          url === undefined
            ? undefined
            : {
                url,
                code: typeof signIn?.code === 'string' ? signIn.code : undefined,
                message: typeof signIn?.message === 'string' ? signIn.message : undefined,
              },
      };
    }
  }

  const mode = fieldOf(current, 'mode');
  if (fieldChanged(previous, current, 'mode') && typeof mode === 'string') {
    next = { ...next, mode };
  }

  const plan = fieldOf(current, 'plan');
  if (fieldChanged(previous, current, 'plan') && Array.isArray(plan)) {
    next = {
      ...next,
      plan: plan.flatMap((entry): PlanStep[] => {
        const value = recordOf(entry);
        const title = typeof value?.title === 'string' ? value.title : '';
        const status = typeof value?.status === 'string' ? value.status : '';
        return title === '' ? [] : [{ title, status }];
      }),
    };
  }

  const configOptions = fieldOf(current, 'configOptions');
  if (fieldChanged(previous, current, 'configOptions') && Array.isArray(configOptions)) {
    next = {
      ...next,
      configOptions: configOptions.flatMap((entry): ConfigOption[] => {
        const value = recordOf(entry);
        const id = typeof value?.id === 'string' ? value.id : '';
        const name = typeof value?.name === 'string' ? value.name : '';
        const kind = typeof value?.kind === 'string' ? value.kind : '';
        return id === '' ? [] : [{ id, name, kind }];
      }),
    };
  }

  const availableCommands = fieldOf(current, 'availableCommands');
  if (fieldChanged(previous, current, 'availableCommands') && Array.isArray(availableCommands)) {
    next = {
      ...next,
      availableCommands: availableCommands.flatMap((entry): AvailableCommand[] => {
        const value = recordOf(entry);
        const name = typeof value?.name === 'string' ? value.name : '';
        const description = typeof value?.description === 'string' ? value.description : '';
        return name === '' ? [] : [{ name, description }];
      }),
    };
  }

  const pendingApprovals = recordOf(fieldOf(current, 'pendingApprovals'));
  if (
    next.pending !== undefined &&
    (pendingApprovals === undefined || !Object.hasOwn(pendingApprovals, next.pending.id))
  ) {
    next = { ...next, pending: undefined };
  }

  return next;
}

function projectGaps(
  state: Conversation,
  previous: AguiState,
  current: AguiState,
  event: Extract<AguiEvent, { type: 'STATE_DELTA' }>,
): Conversation {
  let next = state;
  for (const addition of appendedValues(previous, current, 'gaps')) {
    const value = recordOf(addition.value) ?? {};
    const frameCount = value[GAP_VALUE_FRAMES];
    next = append(next, {
      kind: 'gap',
      id: stateEntryId('gap', event, addition),
      frames:
        typeof frameCount === 'number' && Number.isSafeInteger(frameCount) && frameCount >= 0
          ? frameCount
          : undefined,
      seq: event.seq,
    });
  }
  return next;
}

function projectPlaneChanges(
  state: Conversation,
  previous: AguiState,
  current: AguiState,
  event: Extract<AguiEvent, { type: 'STATE_DELTA' }>,
): Conversation {
  let next = state;
  for (const addition of appendedValues(previous, current, 'planeChanges')) {
    const value = recordOf(addition.value) ?? {};
    const snapshotAt = typeof value.snapshotAt === 'string' ? value.snapshotAt : undefined;
    if (snapshotAt !== undefined && snapshotAt !== '') {
      next = append(next, {
        kind: 'gap',
        id: stateEntryId('plane', event, addition),
        rotatedAt: snapshotAt,
        movedTo: typeof value.to === 'string' ? value.to : undefined,
        seq: event.seq,
      });
    }
  }
  return next;
}

function projectContextCompactions(
  state: Conversation,
  previous: AguiState,
  current: AguiState,
  event: Extract<AguiEvent, { type: 'STATE_DELTA' }>,
): Conversation {
  let next = state;
  for (const addition of appendedValues(previous, current, 'contextCompactions')) {
    const value = recordOf(addition.value) ?? {};
    const was = typeof value.was === 'number' ? value.was : undefined;
    const now = typeof value.now === 'number' ? value.now : undefined;
    if (was !== undefined && now !== undefined) {
      const window =
        typeof value.window === 'number' && value.window > 0 ? value.window : undefined;
      next = append(next, {
        kind: 'contextCompaction',
        id: stateEntryId('context-compaction', event, addition),
        was,
        now,
        window,
        seq: event.seq,
      });
    }
  }
  return next;
}

function projectRuntimeLosses(
  state: Conversation,
  previous: AguiState,
  current: AguiState,
  event: Extract<AguiEvent, { type: 'STATE_DELTA' }>,
): Conversation {
  let next = state;
  for (const addition of appendedValues(previous, current, 'runtimeLosses')) {
    const value = recordOf(addition.value) ?? {};
    const runtime = typeof value.runtime === 'string' ? value.runtime : undefined;
    if (runtime !== undefined) {
      next = {
        ...next,
        runtimeLoss: {
          runtime,
          cause: typeof value.cause === 'string' ? value.cause : undefined,
          noticedAt: typeof value.noticedAt === 'string' ? value.noticedAt : undefined,
          seq: event.seq,
        },
      };
    }
  }
  return next;
}

interface AppendedValue {
  readonly value: unknown;
  readonly index: number;
  readonly ordinal: number;
}

function appendedValues(
  previous: AguiState,
  current: AguiState,
  field: 'gaps' | 'planeChanges' | 'contextCompactions' | 'runtimeLosses',
): readonly AppendedValue[] {
  const before = fieldOf(previous, field);
  const after = fieldOf(current, field);
  if (!Array.isArray(before) || !Array.isArray(after) || after.length < before.length) {
    return [];
  }
  if (before.some((value, index) => !jsonEqual(value, after[index]))) {
    return [];
  }
  return after.slice(before.length).map((value, ordinal) => ({
    value,
    ordinal,
    index: before.length + ordinal,
  }));
}

function stateEntryId(
  prefix: string,
  event: Extract<AguiEvent, { type: 'STATE_DELTA' }>,
  addition: AppendedValue,
): string {
  return event.ephemeral === true || addition.ordinal > 0
    ? `${prefix}-${String(event.seq)}-${String(addition.index)}`
    : `${prefix}-${String(event.seq)}`;
}

function append(state: Conversation, entry: Entry): Conversation {
  const entries = state.entries.some(
    (existing) => existing.kind === entry.kind && existing.id === entry.id,
  )
    ? state.entries
    : [...state.entries, entry];
  return { ...state, entries };
}

function totalOf(input: number | undefined, output: number | undefined): number | undefined {
  if (input === undefined && output === undefined) {
    return undefined;
  }
  return (input ?? 0) + (output ?? 0);
}

function fieldChanged(previous: AguiState, current: AguiState, field: string): boolean {
  return !jsonEqual(fieldOf(previous, field), fieldOf(current, field));
}

function fieldOf(state: AguiState, field: string): unknown {
  return recordOf(state)?.[field];
}

function recordOf(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

/** Structural equality for values that crossed a JSON protocol boundary. */
function jsonEqual(left: unknown, right: unknown): boolean {
  if (left === right || (Number.isNaN(left) && Number.isNaN(right))) {
    return true;
  }
  if (Array.isArray(left) || Array.isArray(right)) {
    return (
      Array.isArray(left) &&
      Array.isArray(right) &&
      left.length === right.length &&
      left.every((value, index) => jsonEqual(value, right[index]))
    );
  }
  const leftRecord = recordOf(left);
  const rightRecord = recordOf(right);
  if (leftRecord === undefined || rightRecord === undefined) {
    return false;
  }
  const leftKeys = Object.keys(leftRecord);
  const rightKeys = Object.keys(rightRecord);
  return (
    leftKeys.length === rightKeys.length &&
    leftKeys.every(
      (key) => Object.hasOwn(rightRecord, key) && jsonEqual(leftRecord[key], rightRecord[key]),
    )
  );
}
