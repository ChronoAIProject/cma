import { useCallback, useEffect, useRef, useState } from 'react';
import type { AgentDraft } from './draft';
import {
  attachmentLimit,
  attachmentSizeLimit,
  type DraftAttachment,
  type UploadAttachment,
} from './attachments';

/** Two uploads at a time; each retry keeps the same logical upload identity. */
export function useComposerAttachments(
  draft: AgentDraft | undefined,
  upload: UploadAttachment | undefined,
  enabled: boolean,
) {
  const [local, setLocal] = useState<readonly DraftAttachment[]>([]);
  const items = draft?.attachments ?? local;
  const setItems = draft?.setAttachments ?? setLocal;
  const [error, setError] = useState<string>();
  const active = useRef(new Map<string, AbortController>());

  useEffect(() => {
    const pending = active.current;
    return () => {
      for (const controller of pending.values()) controller.abort();
      pending.clear();
    };
  }, []);

  useEffect(() => {
    if (!enabled || upload === undefined) {
      for (const controller of active.current.values()) controller.abort();
      active.current.clear();
      return;
    }
    // A remounted composer can retain a draft whose previous request was interrupted.
    const interrupted = items.filter(
      (item) => item.status === 'uploading' && !active.current.has(item.key),
    );
    if (interrupted.length > 0) {
      setItems((current) =>
        current.map((item) =>
          interrupted.some(({ key }) => key === item.key)
            ? { ...item, status: 'error', error: 'Upload interrupted. Retry to continue.' }
            : item,
        ),
      );
      return;
    }
    const next = items
      .filter((item) => item.status === 'queued' && !active.current.has(item.key))
      .slice(0, Math.max(0, 2 - active.current.size));
    for (const item of next) {
      const controller = new AbortController();
      active.current.set(item.key, controller);
      setItems((current) =>
        current.map((entry) =>
          entry.key === item.key ? { ...entry, status: 'uploading' } : entry,
        ),
      );
      void Promise.resolve()
        .then(() => upload(item.file, { idempotencyKey: item.key, signal: controller.signal }))
        .then(
          (uploaded) => {
            if (controller.signal.aborted) return;
            active.current.delete(item.key);
            setItems((current) =>
              current.map((entry) =>
                entry.key === item.key ? { ...entry, status: 'ready', uploaded } : entry,
              ),
            );
          },
          (reason: unknown) => {
            if (controller.signal.aborted) return;
            active.current.delete(item.key);
            setItems((current) =>
              current.map((entry) =>
                entry.key === item.key
                  ? {
                      ...entry,
                      status: 'error',
                      error: reason instanceof Error ? reason.message : 'Upload failed. Try again.',
                    }
                  : entry,
              ),
            );
          },
        );
    }
  }, [enabled, items, setItems, upload]);

  const add = useCallback(
    (files: readonly File[]) => {
      if (files.length === 0) return;
      if (!enabled || upload === undefined) {
        setError('Attachments are available when this sandbox is ready.');
        return;
      }
      if (items.length + files.length > attachmentLimit) {
        setError(`Attach up to ${String(attachmentLimit)} files per message.`);
        return;
      }
      if (files.some((file) => file.size > attachmentSizeLimit)) {
        setError('Each attachment must be 20 MiB or smaller.');
        return;
      }
      setError(undefined);
      const added = files.map((file) => ({
        key: crypto.randomUUID(),
        file,
        status: 'queued' as const,
      }));
      setItems((current) =>
        current.length + added.length <= attachmentLimit ? [...current, ...added] : current,
      );
    },
    [enabled, items.length, setItems, upload],
  );
  const remove = (key: string) => {
    active.current.get(key)?.abort();
    active.current.delete(key);
    setItems((current) => current.filter((item) => item.key !== key));
  };
  const retry = (key: string) => {
    if (!enabled) return;
    setItems((current) =>
      current.map((item) =>
        item.key === key && item.status === 'error'
          ? { ...item, status: 'queued', error: undefined }
          : item,
      ),
    );
  };
  const accept = (submitted: readonly DraftAttachment[]) => {
    const keys = new Set(submitted.map(({ key }) => key));
    setItems((current) => current.filter((item) => !keys.has(item.key)));
  };
  return {
    items,
    error,
    add,
    remove,
    retry,
    accept,
    blocked: items.some((item) => item.status !== 'ready'),
  };
}
