import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ApiError,
  ProtocolError,
  ValidationError,
  type CreateBridgeRequest,
} from '@chronoai/cma-sdk';
import { useAuthorizedRead } from '../resources/useAuthorizedRead';
import type { BridgeViewsClient } from './client';
import { bridgeFailure } from './commands';
import {
  creationStorageKey,
  loadCreationDraft,
  sameOwner,
  saveCreationDraft,
  type CreationDraft,
} from './creationDraft';

type ResourceOwner = CreateBridgeRequest['owner'];

export function useBridgeCreation(
  client: BridgeViewsClient,
  viewer: string,
  organization?: string,
) {
  const storageKey = creationStorageKey(viewer, organization);
  const owner: ResourceOwner =
    organization === undefined
      ? { kind: 'person', subject: viewer }
      : { kind: 'organization', subject: organization };
  const [state, setState] = useState<{ draft?: CreationDraft; storageError?: string }>(() => {
    try {
      return { draft: loadCreationDraft(storageKey, owner) };
    } catch {
      return {
        storageError:
          'This browser could not retain your bridge setup. Restore session storage before continuing.',
      };
    }
  });
  const current = useRef(state.draft);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const active = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => active.current?.abort(), []);
  const retain = useCallback(
    (draft: CreationDraft) => {
      saveCreationDraft(storageKey, draft);
      current.current = draft;
      setState({ draft });
    },
    [storageKey],
  );
  const key = state.draft?.key;
  const read = useMemo(
    () =>
      key === undefined
        ? undefined
        : async (signal: AbortSignal) => {
            try {
              const value = await client.bridges.creationTarget({
                organization,
                idempotencyKey: key,
                signal,
              });
              signal.throwIfAborted();
              const expected: ResourceOwner =
                organization === undefined
                  ? { kind: 'person', subject: viewer }
                  : { kind: 'organization', subject: organization };
              const draft = current.current;
              if (
                draft === undefined ||
                draft.key !== key ||
                !sameOwner(value.owner, expected) ||
                (draft.target !== undefined &&
                  (draft.target.bridge_id !== value.bridge_id ||
                    draft.target.callback_url !== value.callback_url ||
                    !sameOwner(draft.target.owner, value.owner)))
              )
                throw new ProtocolError('Bridge setup identity changed.');
              retain({ ...draft, target: value });
              return { value };
            } catch (failure) {
              throw new Error(bridgeFailure(failure));
            }
          },
    [client, key, organization, viewer, retain],
  );
  const preview = useAuthorizedRead(read);
  const target = preview.state.status === 'ready' ? preview.state.value : undefined;

  const submit = async (
    request?: CreateBridgeRequest,
    prepare?: (signal: AbortSignal) => Promise<void>,
  ) => {
    if (active.current !== undefined || target === undefined || current.current === undefined)
      return;
    const controller = new AbortController();
    const recovering = current.current.request !== undefined;
    active.current = controller;
    setBusy(true);
    setError(undefined);
    const timer = setTimeout(() => controller.abort(), 15_000);
    try {
      let draft = current.current;
      if (draft.request === undefined) {
        if (request === undefined || !sameOwner(request.owner, target.owner))
          throw new ValidationError('Bridge owner changed.');
        // Freeze the exact request before any external checks or eventual dispatch.
        const captured = structuredClone(request);
        await prepare?.(controller.signal);
        controller.signal.throwIfAborted();
        draft = { ...draft, request: captured };
        retain(draft);
      }
      const receipt = await client.bridges.create(draft.request!, {
        idempotencyKey: draft.key,
        signal: controller.signal,
      });
      controller.signal.throwIfAborted();
      if (receipt.bridge_id !== target.bridge_id || !sameOwner(receipt.owner, target.owner))
        throw new ProtocolError('Created bridge did not match its prepared callback.');
      sessionStorage.removeItem(storageKey);
      current.current = undefined;
      setState({});
      return receipt;
    } catch (failure) {
      // Definitive pre-effect refusal can be corrected. Ambiguity keeps the original body and key.
      if (
        !recovering &&
        (failure instanceof ValidationError ||
          (failure instanceof ApiError && !failure.isRetryable))
      ) {
        const draft = current.current;
        if (draft !== undefined) {
          try {
            retain({ version: 1, key: draft.key, target: draft.target });
          } catch {
            /* The original persisted request stays available for recovery. */
          }
        }
      }
      setError(bridgeFailure(failure));
    } finally {
      clearTimeout(timer);
      if (active.current === controller) {
        active.current = undefined;
        setBusy(false);
      }
    }
  };
  return {
    preview,
    target,
    submit,
    busy,
    error: state.storageError ?? error,
    pending: state.draft?.request !== undefined,
    ready: target !== undefined && state.storageError === undefined,
  };
}
