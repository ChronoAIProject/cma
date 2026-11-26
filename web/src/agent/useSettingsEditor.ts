import { useCallback, useEffect, useRef, useState } from 'react';
import { settingsFailure, settingsWriteFailure, type SettingsFailure } from './settingsFailure';
import {
  settingsJournalError,
  type OpaqueSettingsIntent,
  type SettingsIntent,
  type SettingsIntentJournal,
} from './settingsIntent';
import { observeRead } from './transport/readObserver';
import { retrySchedule } from './transport/readRetry';

export interface SettingsEditorAdapter<View, Proposal, Body> {
  read(signal: AbortSignal): Promise<View>;
  update(body: Body, options: { signal: AbortSignal; idempotencyKey: string }): Promise<View>;
  proposal(view: View): Proposal;
  body(view: View, proposal: Proposal): Body;
  equal(left: Proposal, right: Proposal): boolean;
  pending(view: View): boolean;
  readonly mergeFields?: readonly (keyof Proposal)[];
  retention?: {
    journal(): SettingsIntentJournal<Body>;
    proposal(body: Body): Proposal;
    emptyReentry?: Proposal;
  };
}

interface WriteIntent<Proposal, Body> extends SettingsIntent<Body> {
  readonly proposal: Proposal;
  readonly phase: 'sending' | 'uncertain' | 'pending';
}
type ReentryIntent<Body> = OpaqueSettingsIntent<Body> & { readonly phase: 'reenter' };
type Intent<Proposal, Body> = WriteIntent<Proposal, Body> | ReentryIntent<Body>;
interface EditorState<View, Proposal, Body> {
  readonly snapshot?: View;
  readonly proposal?: Proposal;
  readonly editRevision: number;
  readonly editsDuringRead?: ReadonlySet<keyof Proposal>;
  readonly intent?: Intent<Proposal, Body>;
  readonly failure?: SettingsFailure;
  readonly refreshRequired: boolean;
  readonly reading: boolean;
  readonly saving: boolean;
  readonly confirmed: boolean;
  readonly retryAt: number;
  readonly storageBlocked?: boolean;
}

function completeStoredIntent<Body>(journal: SettingsIntentJournal<Body> | undefined, key: string) {
  try {
    journal?.complete(key);
    return true;
  } catch {
    return false;
  }
}

function hideDeniedContent<View, Proposal, Body>(
  state: EditorState<View, Proposal, Body>,
  journal: SettingsIntentJournal<Body> | undefined,
): EditorState<View, Proposal, Body> {
  const hidden = {
    ...state,
    snapshot: undefined,
    proposal: undefined,
    editsDuringRead: undefined,
    intent: undefined,
    failure: state.failure && {
      ...state.failure,
      message: 'Access to these preferences was refused. Refresh to check access before saving.',
    },
  };
  try {
    const retained = journal?.read();
    if (retained && 'recover' in retained)
      return { ...hidden, intent: { ...retained, phase: 'reenter' } };
    return hidden;
  } catch {
    return { ...hidden, storageBlocked: true };
  }
}

function refreshedProposal<View, Proposal, Body>(
  adapter: SettingsEditorAdapter<View, Proposal, Body>,
  state: EditorState<View, Proposal, Body>,
  fresh: Proposal,
  editRevision: number,
): Proposal | undefined {
  const { snapshot, proposal } = state;
  if (snapshot !== undefined && proposal !== undefined) {
    const baseline = adapter.proposal(snapshot);
    if (adapter.mergeFields && proposal != null && baseline != null && fresh != null) {
      const merged = { ...fresh };
      for (const field of adapter.mergeFields) {
        if (proposal[field] !== baseline[field] || state.editsDuringRead?.has(field))
          merged[field] = proposal[field];
      }
      return merged;
    }
    if (!adapter.equal(proposal, baseline)) return proposal;
  }
  return state.editRevision !== editRevision ? proposal : fresh;
}

export function useSettingsEditor<View, Proposal, Body>(
  adapter: SettingsEditorAdapter<View, Proposal, Body>,
  active = true,
) {
  const [state, setState] = useState<EditorState<View, Proposal, Body>>({
    refreshRequired: false,
    editRevision: 0,
    reading: false,
    saving: false,
    confirmed: false,
    retryAt: 0,
  });
  // Callbacks use the same serialized state even before React paints a second click.
  const current = useRef(state);
  const publish = useCallback((change: (previous: typeof state) => typeof state) => {
    current.current = change(current.current);
    setState(current.current);
  }, []);
  const request = useRef<AbortSignal | undefined>(undefined);
  const mutation = useRef<AbortController | undefined>(undefined);
  const journal = useRef<SettingsIntentJournal<Body> | undefined>(undefined);
  const observer = useRef<ReturnType<typeof observeRead> | undefined>(undefined);
  useEffect(() => {
    if (!state.retryAt) return;
    const timer = setTimeout(
      () =>
        publish((previous) =>
          previous.retryAt === state.retryAt ? { ...previous, retryAt: 0 } : previous,
        ),
      Math.max(0, state.retryAt - Date.now()),
    );
    return () => clearTimeout(timer);
  }, [state.retryAt, publish]);
  useEffect(() => {
    if (!state.confirmed) return;
    const timer = setTimeout(
      () => publish((previous) => ({ ...previous, confirmed: false })),
      3000,
    );
    return () => clearTimeout(timer);
  }, [state.confirmed, publish]);
  useEffect(() => {
    if (!active) return;
    try {
      if (adapter.retention) {
        journal.current = adapter.retention.journal();
        const restored = journal.current.read();
        if (restored && !current.current.intent) {
          const intent: Intent<Proposal, Body> =
            'recover' in restored
              ? { ...restored, phase: 'reenter' }
              : {
                  ...restored,
                  proposal: adapter.retention.proposal(restored.body),
                  phase: 'uncertain',
                };
          publish((previous) => ({
            ...previous,
            intent,
            failure: settingsFailure(undefined),
            refreshRequired: true,
            retryAt: restored.retryAt && restored.retryAt > Date.now() ? restored.retryAt : 0,
          }));
        }
      }
    } catch {
      publish((previous) => ({ ...previous, storageBlocked: true }));
    }
    let failures = 0;
    let readSignal: AbortSignal | undefined;
    const read = observeRead(async (signal) => {
      const delay = current.current.retryAt - Date.now();
      if (request.current || delay > 0)
        return { next: Math.max(100, delay), cooldown: Math.max(100, delay) };
      request.current = signal;
      readSignal = signal;
      const editRevision = current.current.editRevision;
      publish((previous) => ({ ...previous, reading: true }));
      try {
        const next = await adapter.read(signal);
        if (signal.aborted) return;
        const proposal = adapter.proposal(next);
        publish((previous) => {
          const intent = previous.intent;
          const confirmed =
            intent?.phase === 'pending' &&
            !adapter.pending(next) &&
            adapter.equal(intent.proposal, proposal);
          return {
            ...previous,
            snapshot: next,
            proposal: intent
              ? (previous.proposal ??
                (intent.phase === 'reenter' ? adapter.retention?.emptyReentry : intent.proposal))
              : refreshedProposal(adapter, previous, proposal, editRevision),
            editsDuringRead: undefined,
            intent: confirmed ? undefined : previous.intent,
            confirmed: confirmed || previous.confirmed,
            storageBlocked:
              previous.storageBlocked ||
              (confirmed && !completeStoredIntent(journal.current, intent.key)),
            failure:
              intent &&
              !confirmed &&
              !(intent.phase === 'reenter' && previous.failure?.kind === 'denied')
                ? previous.failure
                : undefined,
            refreshRequired: false,
          };
        });
        failures = 0;
        if (adapter.pending(next) || current.current.intent?.phase === 'pending')
          return { next: 2000, cooldown: 2000 };
        return { cooldown: 1000 };
      } catch (cause) {
        if (signal.aborted) return;
        const failure = settingsFailure(cause, true);
        publish((previous) => {
          const next = {
            ...previous,
            failure,
            confirmed: false,
            refreshRequired: true,
            retryAt: failure.retryAfterMs ? Date.now() + failure.retryAfterMs : 0,
          };
          return failure.kind === 'denied' && adapter.retention?.emptyReentry !== undefined
            ? hideDeniedContent(next, journal.current)
            : next;
        });
        if (failure.kind === 'denied') return { cooldown: failure.retryAfterMs };
        return retrySchedule(cause, ++failures);
      } finally {
        if (request.current === signal) request.current = undefined;
        if (!signal.aborted) publish((previous) => ({ ...previous, reading: false }));
      }
    });
    observer.current = read;
    return () => {
      read.dispose();
      journal.current = undefined;
      const write = mutation.current;
      write?.abort();
      if (request.current === readSignal || request.current === write?.signal)
        request.current = undefined;
      mutation.current = undefined;
      if (observer.current === read) observer.current = undefined;
      publish((previous) => ({
        ...previous,
        reading: false,
        saving: false,
        ...(previous.intent?.phase === 'sending'
          ? {
              intent: { ...previous.intent, phase: 'uncertain' as const },
              failure: settingsFailure(undefined),
              refreshRequired: true,
            }
          : {}),
      }));
    };
  }, [adapter, active, publish]);

  const apply = async (): Promise<View | undefined> => {
    const previous = current.current;
    const { snapshot, proposal } = previous;
    if (
      !active ||
      request.current ||
      snapshot === undefined ||
      proposal === undefined ||
      previous.refreshRequired ||
      previous.storageBlocked ||
      previous.intent?.phase === 'pending' ||
      adapter.pending(snapshot) ||
      Date.now() < previous.retryAt
    )
      return;
    const controller = new AbortController();
    mutation.current = controller;
    request.current = controller.signal;
    publish((value) => ({
      ...value,
      saving: true,
      failure: undefined,
      confirmed: false,
    }));
    try {
      let retained: WriteIntent<Proposal, Body>;
      try {
        const intent = previous.intent;
        const candidate = () => adapter.body(snapshot, proposal);
        const body =
          intent?.phase === 'reenter'
            ? await intent.recover(candidate())
            : (intent?.body ?? candidate());
        if (controller.signal.aborted) return;
        if (body === undefined) {
          const failure = {
            kind: 'invalid',
            message: 'Enter the original system prompt exactly to retry this request.',
            retryAfterMs: 0,
          } satisfies SettingsFailure;
          publish((value) => ({ ...value, failure }));
          return;
        }
        retained = {
          key: intent?.key ?? crypto.randomUUID(),
          startedAt: intent?.startedAt ?? Date.now(),
          body,
          proposal: intent && intent.phase !== 'reenter' ? intent.proposal : proposal,
          phase: 'sending',
        };
        const reservation = journal.current?.reserve(
          { key: retained.key, body: retained.body, startedAt: retained.startedAt },
          controller.signal,
        );
        if (reservation) await reservation;
        if (controller.signal.aborted) return;
      } catch {
        if (!controller.signal.aborted) publish((value) => ({ ...value, storageBlocked: true }));
        return;
      }
      publish((value) => ({ ...value, intent: retained }));
      const next = await adapter.update(retained.body, {
        signal: controller.signal,
        idempotencyKey: retained.key,
      });
      if (controller.signal.aborted) return;
      const pending = adapter.pending(next);
      const storageBlocked = !pending && !completeStoredIntent(journal.current, retained.key);
      publish((value) => ({
        ...value,
        snapshot: next,
        proposal:
          value.proposal !== undefined && adapter.equal(value.proposal, retained.proposal)
            ? adapter.proposal(next)
            : value.proposal,
        intent: pending
          ? { ...retained, proposal: adapter.proposal(next), phase: 'pending' }
          : undefined,
        confirmed: !pending,
        storageBlocked,
        retryAt: 0,
      }));
      if (pending) observer.current?.refresh();
      return next;
    } catch (cause) {
      if (controller.signal.aborted) return;
      const retained = current.current.intent;
      if (retained?.phase !== 'sending') return;
      const failure = settingsWriteFailure(cause, previous.intent !== undefined);
      const retryAt = failure.retryAfterMs ? Date.now() + failure.retryAfterMs : 0;
      let storageBlocked = false;
      if (failure.outcome === 'rejected') {
        storageBlocked = !completeStoredIntent(journal.current, retained.key);
      } else if (retryAt) {
        try {
          journal.current?.defer(retained.key, retryAt);
        } catch {
          storageBlocked = true;
        }
      }
      publish((value) => {
        const next: EditorState<View, Proposal, Body> = {
          ...value,
          failure,
          storageBlocked,
          intent:
            failure.outcome === 'unconfirmed' ? { ...retained, phase: 'uncertain' } : undefined,
          refreshRequired: failure.refreshRequired,
          retryAt,
        };
        return failure.kind === 'denied' && adapter.retention?.emptyReentry !== undefined
          ? hideDeniedContent(next, journal.current)
          : next;
      });
    } finally {
      if (request.current === controller.signal) request.current = undefined;
      if (mutation.current === controller) mutation.current = undefined;
      if (!controller.signal.aborted) publish((value) => ({ ...value, saving: false }));
    }
  };
  const edit = (proposal: Proposal) => {
    if (
      current.current.saving ||
      (current.current.intent && current.current.intent.phase !== 'reenter')
    )
      return;
    publish((previous) => {
      const before = previous.proposal;
      let editsDuringRead = previous.editsDuringRead;
      if (previous.reading && before != null && proposal != null && adapter.mergeFields) {
        // A field changed during the read can return to its old value while the
        // server changes it concurrently. Keep that edit until a read succeeds.
        const changed = adapter.mergeFields.filter((field) => before[field] !== proposal[field]);
        editsDuringRead = new Set([...(editsDuringRead ?? []), ...changed]);
      }
      return {
        ...previous,
        proposal,
        editRevision: previous.editRevision + 1,
        editsDuringRead,
        confirmed: false,
      };
    });
  };
  const waiting = state.retryAt > 0;
  const pending =
    state.intent?.phase === 'pending' ||
    (state.snapshot !== undefined && adapter.pending(state.snapshot));
  return {
    ...state,
    failure: state.storageBlocked
      ? ({
          kind: 'retry',
          message: settingsJournalError,
          retryAfterMs: 0,
        } satisfies SettingsFailure)
      : state.failure,
    edit,
    apply,
    refresh: () => observer.current?.refresh(),
    waiting,
    pending,
    busy: state.reading || state.saving,
    locked:
      state.saving || (state.intent !== undefined && state.intent.phase !== 'reenter') || pending,
    canApply:
      active &&
      !state.reading &&
      !state.saving &&
      !state.refreshRequired &&
      !state.storageBlocked &&
      !waiting &&
      !pending,
  };
}
