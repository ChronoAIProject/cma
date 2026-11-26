import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import { retainPreparationDraftOwner } from './preparationDraftIdentity';
import {
  preparationDraftKey,
  preparationDraftReadError,
  preparationDraftWriteError,
  readPreparationDraft,
  savePreparationDraft,
  samePreparationBinding,
  type PreparationDraftBinding,
  type PreparationDraftRecord,
  type PreparationDraftScope,
} from './preparationDraftStorage';

interface Entry {
  readonly text: string;
  readonly record?: PreparationDraftRecord;
  readonly error?: string;
  readonly unread?: boolean;
}
function restore(scope?: PreparationDraftScope): Entry {
  if (!scope) return { text: '' };
  try {
    const record = readPreparationDraft(scope);
    return { record, text: record?.text ?? '' };
  } catch {
    return { text: '', error: preparationDraftReadError, unread: true };
  }
}

export function usePreparationDraft(client: CmaWebClient, scope?: PreparationDraftScope) {
  const key = scope && preparationDraftKey(scope);
  const [state, setState] = useState(() => {
    if (scope) retainPreparationDraftOwner(scope.owner);
    return { client, key, scope, entry: restore(scope) };
  });
  const matches = state.client === client && state.key === key;
  if (!matches) {
    if (scope && state.scope?.owner !== scope.owner) retainPreparationDraftOwner(scope.owner);
    setState({ client, key, scope, entry: state.key === key ? state.entry : restore(scope) });
  }
  const active = useRef(state);
  const mounted = useRef(true);
  useLayoutEffect(() => {
    active.current = state;
  }, [state]);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const update = (next: Entry) => {
    if (!mounted.current || active.current.client !== client || active.current.key !== key) return;
    const previous = active.current.entry;
    if (
      previous.text === next.text &&
      previous.record === next.record &&
      previous.error === next.error &&
      previous.unread === next.unread
    )
      return;
    const value = { ...active.current, entry: next };
    active.current = value;
    setState(value);
  };
  const current = () =>
    mounted.current && active.current.client === client && active.current.key === key
      ? active.current.entry
      : undefined;
  const change = (text: string) => {
    const entry = current();
    if (!scope || !entry) return;
    if (entry.unread) {
      update({ ...entry, text });
      return;
    }
    try {
      const record = savePreparationDraft(scope, entry.record, text);
      update({ text, record });
    } catch {
      update({ ...entry, text, error: preparationDraftWriteError });
    }
  };
  const retryStorage = () => {
    const entry = current();
    if (!scope || !entry) return;
    if (!entry.unread) {
      change(entry.text);
      return;
    }
    const saved = restore(scope);
    if (entry.text) {
      update({
        ...entry,
        error:
          saved.error ??
          'Your saved draft is unchanged. Copy your current text, then reload to recover it.',
      });
    } else update(saved);
  };
  const bind = (binding: PreparationDraftBinding) => {
    const entry = current();
    if (!scope || !entry) return false;
    if (!entry.text && !entry.record && !entry.unread) return true;
    if (entry.unread) return false;
    try {
      if (
        entry.record?.binding &&
        samePreparationBinding(entry.record.binding, binding) &&
        !entry.error &&
        JSON.stringify(readPreparationDraft(scope)) === JSON.stringify(entry.record)
      )
        return true;
      const record = savePreparationDraft(scope, entry.record, entry.text, binding);
      update({ text: entry.text, record });
      return true;
    } catch {
      update({ ...entry, error: preparationDraftWriteError });
      return false;
    }
  };
  return { ...(matches ? state.entry : { text: '' }), change, retryStorage, bind };
}
