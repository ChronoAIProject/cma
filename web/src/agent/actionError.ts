import {
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
  type SetStateAction,
  type Dispatch,
  type RefObject,
} from 'react';
import type { AgentV2Session } from './transport/client';
import type { AgentCommand, AgentResourceState } from './controllerTypes';
import { IdentityChangedError, isIdentityChanged } from './transport/identity';

export interface InputErrorOwner {
  readonly session: AgentV2Session;
  readonly key: string;
}

export type SetActionError = (
  message: SetStateAction<string | undefined>,
  input?: InputErrorOwner,
) => void;

interface ActionError {
  readonly revision: symbol;
  readonly message: string;
  readonly input?: InputErrorOwner;
}

function pendingOwner(input: InputErrorOwner | undefined) {
  try {
    const retained = input?.session.journal.read();
    return retained?.state === 'pending' && retained.key === input?.key ? input : undefined;
  } catch {
    return undefined;
  }
}

export function useAgentActionError() {
  const [error, setError] = useState<ActionError>();
  const latest = useRef(error);
  useLayoutEffect(() => {
    latest.current = error;
  }, [error]);
  const setActionError: SetActionError = useCallback((value, input) => {
    const owner = pendingOwner(input);
    const revision = Symbol();
    setError((current) => {
      const message = typeof value === 'function' ? value(current?.message) : value;
      return message === undefined ? undefined : { message, input: owner, revision };
    });
  }, []);
  const recoverInputError = useCallback((session: AgentV2Session, key: string) => {
    const expected = latest.current;
    return () => {
      try {
        const accepted = session.journal.read();
        if (accepted?.state !== 'accepted' || accepted.key !== key) return;
      } catch {
        return;
      }
      setError((current) =>
        current?.revision === expected?.revision &&
        expected?.input?.session === session &&
        expected.input.key === key
          ? undefined
          : current,
      );
    };
  }, []);
  return [error?.message, setActionError, recoverInputError] as const;
}

export function useAgentIdentityChange(
  actionAbort: RefObject<AbortController | undefined>,
  replayAbort: RefObject<AbortController | undefined>,
  setPending: Dispatch<SetStateAction<AgentCommand | undefined>>,
  setActionError: SetActionError,
  setStreamError: Dispatch<SetStateAction<string | undefined>>,
  setState: Dispatch<SetStateAction<AgentResourceState>>,
) {
  return useCallback(
    (error: unknown) => {
      if (!isIdentityChanged(error)) return false;
      actionAbort.current?.abort();
      replayAbort.current?.abort();
      setPending(undefined);
      setActionError(undefined);
      setStreamError(undefined);
      setState({ status: 'error', message: new IdentityChangedError().message });
      return true;
    },
    [actionAbort, replayAbort, setPending, setActionError, setStreamError, setState],
  );
}
