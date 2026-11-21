import { useCallback, useEffect, useRef, useState } from 'react';
import { settingsFailure, settingsWriteFailure, type SettingsFailure } from './settingsFailure';
import {
  settingsJournalError,
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
  retention?: {
    journal(): SettingsIntentJournal<Body>;
    proposal(body: Body): Proposal;
  };
}

interface Intent<Proposal, Body> extends SettingsIntent<Body> {
  readonly proposal: Proposal;
  readonly phase: 'sending' | 'uncertain' | 'pending';
}
interface EditorState<View, Proposal, Body> {
  readonly snapshot?: View;
  readonly proposal?: Proposal;
  readonly editRevision: number;
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
          const proposal = adapter.retention.proposal(restored.body);
          publish((previous) => ({
            ...previous,
            intent: { ...restored, proposal, phase: 'uncertain' },
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
          const dirty =
            previous.editRevision !== editRevision ||
            (previous.proposal !== undefined &&
              previous.snapshot !== undefined &&
              !adapter.equal(previous.proposal, adapter.proposal(previous.snapshot)));
          const intent = previous.intent;
          const confirmed =
            intent?.phase === 'pending' &&
            !adapter.pending(next) &&
            adapter.equal(intent.proposal, proposal);
          return {
            ...previous,
            snapshot: next,
            proposal: intent
              ? (previous.proposal ?? intent.proposal)
              : dirty
                ? previous.proposal
                : proposal,
            intent: confirmed ? undefined : previous.intent,
            confirmed: confirmed || previous.confirmed,
            storageBlocked:
              previous.storageBlocked ||
              (confirmed && !completeStoredIntent(journal.current, intent.key)),
            failure: previous.intent && !confirmed ? previous.failure : undefined,
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
        publish((previous) => ({
          ...previous,
          failure,
          confirmed: false,
          refreshRequired: true,
          retryAt: failure.retryAfterMs ? Date.now() + failure.retryAfterMs : 0,
        }));
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
    if (
      !active ||
      request.current ||
      previous.snapshot === undefined ||
      previous.proposal === undefined ||
      previous.refreshRequired ||
      previous.storageBlocked ||
      previous.intent?.phase === 'pending' ||
      adapter.pending(previous.snapshot) ||
      Date.now() < previous.retryAt
    )
      return;
    const retained = previous.intent ?? {
      key: crypto.randomUUID(),
      startedAt: Date.now(),
      body: adapter.body(previous.snapshot, previous.proposal),
      proposal: previous.proposal,
      phase: 'sending' as const,
    };
    try {
      journal.current?.reserve({
        key: retained.key,
        body: retained.body,
        startedAt: retained.startedAt,
      });
    } catch {
      publish((value) => ({ ...value, storageBlocked: true }));
      return;
    }
    const controller = new AbortController();
    mutation.current = controller;
    request.current = controller.signal;
    publish((value) => ({
      ...value,
      intent: { ...retained, phase: 'sending' },
      saving: true,
      failure: undefined,
      confirmed: false,
    }));
    try {
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
      publish((value) => ({
        ...value,
        failure,
        storageBlocked,
        intent: failure.outcome === 'unconfirmed' ? { ...retained, phase: 'uncertain' } : undefined,
        refreshRequired: failure.refreshRequired,
        retryAt,
      }));
    } finally {
      if (request.current === controller.signal) request.current = undefined;
      if (mutation.current === controller) mutation.current = undefined;
      if (!controller.signal.aborted) publish((value) => ({ ...value, saving: false }));
    }
  };
  const edit = (proposal: Proposal) => {
    if (current.current.intent) return;
    publish((previous) => ({
      ...previous,
      proposal,
      editRevision: previous.editRevision + 1,
      confirmed: false,
    }));
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
    locked: state.intent !== undefined || pending,
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
