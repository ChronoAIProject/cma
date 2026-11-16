import { ApiError, type AccountDeletionReceipt } from '@chronoai/cma-sdk';
import { useEffect, useRef, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { RiskConfirmation } from '../ui/RiskConfirmation';
import { readableError, refusalSentence } from '../vocabulary/errors';
import {
  accountDeletionReadFailureCopy,
  clearAccountDeletion,
  isAccountDeletionInProgress,
  persistAccountDeletion,
  readAccountDeletion,
  requestAccountDeletion,
  type AccountDeletionRequest,
  notifyAccountDeletionInProgress,
} from './deletion';
import { AccountDeletionProgress } from './AccountDeletionProgress';

const CONFIRMATION_COPY =
  'Deleting your CMA account permanently removes your Sandboxes, Workspaces, Agent conversations, documents, shares, Triggers, Bots, and credentials. This deletes only CMA data; it does not delete your NyxID account. Deletion is irreversible. Access is revoked immediately when the request is accepted. Live CMA data cleanup must finish within 7 days of the request.';
// The #1499 privacy and retention page is not published yet, so no destination is rendered here.

export function AccountDeletionDialog({
  client,
  viewerId,
  initial,
  restored = false,
  onClose,
  onAccepted,
  onPending,
  onCompleted,
}: {
  readonly client: CmaWebClient;
  readonly viewerId: string;
  readonly initial: AccountDeletionRequest;
  readonly restored?: boolean;
  readonly onClose: () => void;
  readonly onAccepted: (request: AccountDeletionRequest) => void;
  readonly onPending: () => void;
  readonly onCompleted?: () => void;
}) {
  const accountDeletion = client.accountDeletion;
  const [request, setRequest] = useState(initial);
  const [started, setStarted] = useState(restored || initial.accepted);
  const [sending, setSending] = useState(false);
  const [checking, setChecking] = useState(false);
  const [receipt, setReceipt] = useState<AccountDeletionReceipt>();
  const [readError, setReadError] = useState<string>();
  const [commandError, setCommandError] = useState<string>();
  const inFlight = useRef(false);
  const mounted = useRef(true);
  const acceptedChange = useRef(onAccepted);

  useEffect(() => {
    acceptedChange.current = onAccepted;
  }, [onAccepted]);

  useEffect(
    () => () => {
      mounted.current = false;
    },
    [],
  );

  useEffect(() => {
    if (!started || sending || checking || accountDeletion === undefined) return;
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let reading = false;
    let stopped = false;
    const poll = () => {
      if (stopped || reading || controller.signal.aborted || document.hidden) return;
      reading = true;
      void readAccountDeletion(accountDeletion, request, controller.signal)
        .then((next) => {
          if (controller.signal.aborted || stopped) return;
          setReceipt(next);
          setReadError(undefined);
          if (!request.accepted) {
            const accepted = { ...request, accepted: true };
            persistAccountDeletion(viewerId, accepted);
            setRequest(accepted);
            acceptedChange.current(accepted);
          }
          if (next.status === 'completed') clearAccountDeletion(viewerId, request.key);
          if (next.status !== 'completed') timer = setTimeout(poll, 1500);
        })
        .catch((error: unknown) => {
          if (controller.signal.aborted || stopped) return;
          if (isAccountDeletionInProgress(error)) {
            notifyAccountDeletionInProgress(error);
            return;
          }
          setReadError(accountDeletionReadFailureCopy());
          timer = setTimeout(poll, 5000);
        })
        .finally(() => {
          reading = false;
        });
    };
    const onVisibility = () => {
      if (!document.hidden) poll();
    };
    document.addEventListener('visibilitychange', onVisibility);
    poll();
    return () => {
      stopped = true;
      controller.abort();
      clearTimeout(timer);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [accountDeletion, checking, request, sending, started, viewerId]);

  const submit = async () => {
    if (inFlight.current) return;
    if (accountDeletion === undefined) {
      setCommandError('Account deletion is temporarily unavailable. Try again later.');
      return;
    }
    inFlight.current = true;
    setSending(true);
    setCommandError(undefined);
    const submitted = { ...request, accepted: false };
    persistAccountDeletion(viewerId, submitted);
    setRequest(submitted);
    setStarted(true);
    try {
      const next = await requestAccountDeletion(accountDeletion, submitted);
      if (!mounted.current) return;
      const accepted = { ...submitted, accepted: true };
      persistAccountDeletion(viewerId, accepted);
      setRequest(accepted);
      setReceipt(next);
      acceptedChange.current(accepted);
    } catch (error: unknown) {
      if (isAccountDeletionInProgress(error)) {
        persistAccountDeletion(viewerId, submitted);
        setRequest(submitted);
        acceptedChange.current(submitted);
        notifyAccountDeletionInProgress(error);
      } else {
        setCommandError(
          error instanceof ApiError
            ? refusalSentence(error)
            : readableError(error, 'The account deletion request could not be confirmed.'),
        );
      }
    } finally {
      inFlight.current = false;
      if (mounted.current) setSending(false);
    }
  };

  const checkRequest = async () => {
    if (request.accepted) {
      if (checking || sending || accountDeletion === undefined) return;
      setChecking(true);
      setCommandError(undefined);
      try {
        const next = await readAccountDeletion(accountDeletion, request);
        if (!mounted.current) return;
        setReceipt(next);
        setReadError(undefined);
        if (next.status === 'completed') clearAccountDeletion(viewerId, request.key);
      } catch (error: unknown) {
        if (!mounted.current) return;
        if (isAccountDeletionInProgress(error)) notifyAccountDeletionInProgress(error);
        else setReadError(accountDeletionReadFailureCopy());
      } finally {
        if (mounted.current) setChecking(false);
      }
      return;
    }
    await submit();
  };

  const close = () => {
    if (receipt?.status === 'completed') onCompleted?.();
    else if (started) onPending();
    else clearAccountDeletion(viewerId, request.key);
    onClose();
  };

  if (!started)
    return (
      <RiskConfirmation
        open
        title="Delete account"
        description={CONFIRMATION_COPY}
        acknowledgeLabel="I understand that this irreversible action deletes my CMA data."
        confirmLabel="Delete account"
        busy={sending}
        confirmDisabled={accountDeletion === undefined}
        status={
          accountDeletion === undefined ? 'Account deletion is temporarily unavailable.' : undefined
        }
        failure={commandError}
        onCancel={close}
        onConfirm={() => void submit()}
      />
    );

  return (
    <Modal
      open
      title="Delete account"
      dismissible={!sending}
      onClose={close}
      footer={
        <Button disabled={sending} onClick={close}>
          Close
        </Button>
      }
    >
      <AccountDeletionProgress
        receipt={receipt}
        accepted={request.accepted}
        readError={readError}
        checking={sending || checking}
        onCheckRequest={() => void checkRequest()}
      />
      {commandError === undefined ? null : <p role="alert">{commandError}</p>}
    </Modal>
  );
}
