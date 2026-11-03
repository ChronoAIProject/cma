import { useState } from 'react';
import { requireTaskStorage } from './taskIdentity';

export function useTaskDraft(owner?: string, workspaceId?: string, sandboxId?: string) {
  const key = `cma:task-draft:${JSON.stringify([location.origin, owner, workspaceId ?? null, sandboxId ?? null])}`;
  const [initial] = useState(() => {
    try {
      requireTaskStorage();
      if (!owner) throw new Error();
      const raw = sessionStorage.getItem(key);
      if (raw === null) return { text: '' };
      const value = JSON.parse(raw) as { version?: unknown; text?: unknown };
      if (value.version !== 1 || typeof value.text !== 'string') throw new Error();
      return { text: value.text };
    } catch {
      return {
        text: '',
        error:
          'Your saved task could not be read. Restore browser session storage before starting a task.',
      };
    }
  });
  const [text, setText] = useState(initial.text);
  const [error, setError] = useState(initial.error);
  const persist = (value: string) => {
    try {
      requireTaskStorage();
      if (!owner) throw new Error();
      sessionStorage.setItem(key, JSON.stringify({ version: 1, text: value }));
      setError(undefined);
      return true;
    } catch {
      setError(
        'Your task could not be saved. Copy your text and restore browser session storage before starting a task.',
      );
      return false;
    }
  };
  return {
    text,
    error,
    persist: () => persist(text),
    change: (value: string) => {
      setText(value);
      persist(value);
    },
  };
}
