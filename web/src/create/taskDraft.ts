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
      const value = JSON.parse(raw) as {
        version?: unknown;
        text?: unknown;
        repositoryUrl?: unknown;
        repositoryRef?: unknown;
        desktop?: unknown;
      };
      if (
        value.version !== 1 ||
        typeof value.text !== 'string' ||
        (value.repositoryUrl !== undefined && typeof value.repositoryUrl !== 'string') ||
        (value.repositoryRef !== undefined && typeof value.repositoryRef !== 'string') ||
        (value.desktop !== undefined && typeof value.desktop !== 'boolean')
      )
        throw new Error();
      return {
        text: value.text,
        repositoryUrl: value.repositoryUrl,
        repositoryRef: value.repositoryRef,
        desktop: value.desktop,
      };
    } catch {
      return {
        text: '',
        error:
          'Your saved task could not be read. Restore browser session storage before starting a task.',
      };
    }
  });
  const [text, setText] = useState(initial.text);
  const [repositoryUrl, setRepositoryUrl] = useState(initial.repositoryUrl ?? '');
  const [repositoryRef, setRepositoryRef] = useState(initial.repositoryRef ?? '');
  const [error, setError] = useState(initial.error);
  const [desktop, setDesktop] = useState(initial.desktop ?? false);
  const persist = (
    value: string,
    url = repositoryUrl,
    reference = repositoryRef,
    enabled = desktop,
  ) => {
    try {
      requireTaskStorage();
      if (!owner) throw new Error();
      sessionStorage.setItem(
        key,
        JSON.stringify({
          version: 1,
          text: value,
          repositoryUrl: url,
          repositoryRef: reference,
          desktop: enabled,
        }),
      );
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
    repositoryUrl,
    repositoryRef,
    desktop,
    changeDesktop: (enabled: boolean) => {
      setDesktop(enabled);
      persist(text, repositoryUrl, repositoryRef, enabled);
    },
    changeRepository: (url: string, reference: string) => {
      setRepositoryUrl(url);
      setRepositoryRef(reference);
      persist(text, url, reference);
    },
    error,
    persist: () => persist(text),
    change: (value: string) => {
      setText(value);
      persist(value);
    },
  };
}
