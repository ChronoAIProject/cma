import { useCallback, useEffect, useRef, useState } from 'react';
import type { AccountDeletionReceipt } from '@chronoai/cma-sdk';
import type { CmaWebClient } from '../api/client';
import { Button } from '../ui/Button';
import {
  accountDeletionReadFailureCopy,
  accountDeletionSessionEndedCopy,
  clearAccountDeletion,
  isAccountDeletionNotFound,
  isAccountDeletionUnauthorized,
  persistAccountDeletion,
  readAccountDeletion,
  requestAccountDeletion,
  restoreAccountDeletion,
  type AccountDeletionRequest,
} from './deletion';
import { AccountDeletionProgress } from './AccountDeletionProgress';

const EMPTY_STATUS_REQUEST: AccountDeletionRequest = { key: '', accepted: true };

type FailureResult = 'retry' | 'stop';

export function AccountDeletionPending({
  client,
  viewerId,
  onSignOut,
  onCompleted,
  showProgress = true,
}: {
  readonly client: CmaWebClient;
  readonly viewerId: string;
  readonly onSignOut: () => void;
  readonly onCompleted?: () => void;
  readonly showProgress?: boolean;
}) {
  const accountDeletion = client.accountDeletion;
  const [request, setRequest] = useState<AccountDeletionRequest | undefined>(() =>
    restoreAccountDeletion(viewerId),
  );
  const [receipt, setReceipt] = useState<AccountDeletionReceipt>();
  const [readError, setReadError] = useState<string>();
  const [sessionEnded, setSessionEnded] = useState(false);
  const [checking, setChecking] = useState(false);
  const reading = useRef(false);
  const mounted = useRef(true);
  const stopped = useRef(false);
  const replayedKey = useRef<string | undefined>(undefined);
  const checkController = useRef<AbortController | undefined>(undefined);
  const completionChange = useRef(onCompleted);
  const statusRequest = request ?? EMPTY_STATUS_REQUEST;

  useEffect(() => {
    completionChange.current = onCompleted;
  }, [onCompleted]);

  useEffect(
    () => () => {
      mounted.current = false;
      stopped.current = true;
      checkController.current?.abort();
    },
    [],
  );

  const complete = useCallback(
    (next: AccountDeletionReceipt) => {
      if (!mounted.current) return;
      setReceipt(next);
      setReadError(undefined);
      if (next.status === 'completed') {
        clearAccountDeletion(viewerId, request?.key);
        completionChange.current?.();
      }
    },
    [request, viewerId],
  );

  const stopForSessionEnd = useCallback(() => {
    if (!mounted.current) return;
    stopped.current = true;
    clearAccountDeletion(viewerId, request?.key);
    setSessionEnded(true);
    setReadError(accountDeletionSessionEndedCopy());
  }, [request, viewerId]);

  const reconcileNotFound = useCallback(
    async (signal?: AbortSignal): Promise<FailureResult> => {
      if (
        request?.accepted === false &&
        replayedKey.current !== request.key &&
        accountDeletion !== undefined
      ) {
        replayedKey.current = request.key;
        try {
          const next = await requestAccountDeletion(accountDeletion, request, signal);
          if (!mounted.current) return 'stop';
          const accepted = { ...request, accepted: true };
          persistAccountDeletion(viewerId, accepted);
          setRequest(accepted);
          complete(next);
          return next.status === 'completed' ? 'stop' : 'retry';
        } catch (error: unknown) {
          if (isAccountDeletionUnauthorized(error)) {
            stopForSessionEnd();
            return 'stop';
          }
          if (!isAccountDeletionNotFound(error)) {
            if (mounted.current) setReadError(accountDeletionReadFailureCopy());
            return 'retry';
          }
        }
      }
      clearAccountDeletion(viewerId, request?.key);
      completionChange.current?.();
      return 'stop';
    },
    [accountDeletion, complete, request, stopForSessionEnd, viewerId],
  );

  const handleFailure = useCallback(
    async (error: unknown, signal?: AbortSignal): Promise<FailureResult> => {
      if (isAccountDeletionUnauthorized(error)) {
        stopForSessionEnd();
        return 'stop';
      }
      if (isAccountDeletionNotFound(error)) return reconcileNotFound(signal);
      if (mounted.current) setReadError(accountDeletionReadFailureCopy());
      return 'retry';
    },
    [reconcileNotFound, stopForSessionEnd],
  );

  const check = () => {
    if (reading.current || accountDeletion === undefined || sessionEnded || !showProgress) return;
    reading.current = true;
    setChecking(true);
    const controller = new AbortController();
    checkController.current = controller;
    const operation =
      request?.accepted === false
        ? requestAccountDeletion(accountDeletion, request, controller.signal)
        : readAccountDeletion(accountDeletion, statusRequest, controller.signal);
    void operation
      .then((next) => {
        complete(next);
        if (request?.accepted === false && mounted.current) {
          const accepted = { ...request, accepted: true };
          persistAccountDeletion(viewerId, accepted);
          setRequest(accepted);
        }
      })
      .catch((error: unknown) => handleFailure(error, controller.signal))
      .finally(() => {
        reading.current = false;
        if (checkController.current === controller) checkController.current = undefined;
        if (mounted.current) setChecking(false);
      });
  };

  useEffect(() => {
    if (accountDeletion === undefined || sessionEnded || !showProgress) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const controller = new AbortController();
    const poll = () => {
      if (stopped.current || reading.current || controller.signal.aborted || document.hidden)
        return;
      reading.current = true;
      void readAccountDeletion(accountDeletion, statusRequest, controller.signal)
        .then((next) => {
          if (stopped.current || controller.signal.aborted) return;
          complete(next);
          if (request?.accepted === false && mounted.current) {
            const accepted = { ...request, accepted: true };
            persistAccountDeletion(viewerId, accepted);
            setRequest(accepted);
          }
          if (next.status !== 'completed') timer = setTimeout(poll, 1500);
        })
        .catch(async (error: unknown) => {
          if (stopped.current || controller.signal.aborted) return;
          const result = await handleFailure(error, controller.signal);
          if (result === 'retry' && !stopped.current && !controller.signal.aborted)
            timer = setTimeout(poll, 5000);
        })
        .finally(() => {
          reading.current = false;
        });
    };
    const onVisibility = () => {
      if (!document.hidden) poll();
    };
    document.addEventListener('visibilitychange', onVisibility);
    poll();
    return () => {
      controller.abort();
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [
    accountDeletion,
    complete,
    handleFailure,
    request,
    sessionEnded,
    showProgress,
    statusRequest,
    viewerId,
  ]);

  return (
    <main className="app-error" aria-hidden={showProgress ? undefined : true}>
      <h1 className="app-error__title" id="account-deletion-title">
        Deletion in progress
      </h1>
      {showProgress && !sessionEnded ? (
        <section aria-labelledby="account-deletion-title">
          <AccountDeletionProgress
            receipt={receipt}
            accepted={request?.accepted !== false}
            readError={readError}
            checking={checking}
            onCheckRequest={check}
          />
        </section>
      ) : null}
      {sessionEnded ? <p role="status">{accountDeletionSessionEndedCopy()}</p> : null}
      <Button variant="outline" onClick={onSignOut}>
        Sign out
      </Button>
    </main>
  );
}
