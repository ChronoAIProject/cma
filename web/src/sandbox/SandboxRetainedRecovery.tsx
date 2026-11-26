import { useEffect, useRef, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import type { SandboxLifecycle } from '../api/types';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { useRetainedRecovery } from './useRetainedRecovery';

export function SandboxRetainedRecovery({
  client,
  sandboxId,
  ownerId,
  revision,
  disabled,
  failed = true,
  stale = false,
  lifecycle,
  onAccepted,
}: {
  readonly client: CmaWebClient;
  readonly sandboxId: string;
  readonly ownerId: string;
  readonly revision: number;
  readonly disabled: boolean;
  readonly failed?: boolean;
  readonly stale?: boolean;
  readonly lifecycle?: SandboxLifecycle;
  readonly onAccepted: () => void;
}) {
  const failureRevision =
    failed && !stale && !lifecycle?.operation && !lifecycle?.result?.continuing
      ? revision
      : undefined;
  const command = useRetainedRecovery(
    client,
    sandboxId,
    ownerId,
    !disabled && !stale,
    failureRevision,
    onAccepted,
  );
  const [confirmation, setConfirmation] = useState<{
    sandboxId: string;
    ownerId: string;
    revision: number;
  }>();
  const [message, setMessage] = useState<string>();
  const [checking, setChecking] = useState(false);
  const inspection = useRef<AbortController | undefined>(undefined);
  useEffect(
    () => () => {
      inspection.current?.abort();
      inspection.current = undefined;
    },
    [client, sandboxId, ownerId],
  );
  const inspect = async () => {
    if (disabled || stale || checking || !client.inspectSandboxRecovery) return;
    const request = new AbortController();
    inspection.current = request;
    setChecking(true);
    setMessage(undefined);
    const timeout = setTimeout(() => request.abort(), 30_000);
    try {
      const assessment = await client.inspectSandboxRecovery(sandboxId, { signal: request.signal });
      request.signal.throwIfAborted();
      if (
        assessment.admission_enabled &&
        ['recorded_boot_present', 'exact_boot_replacement', 'exact_suspended_runtime'].includes(
          assessment.disposition,
        )
      ) {
        setConfirmation({ sandboxId, ownerId, revision });
      } else {
        setMessage(
          'Recovery is not available for this Sandbox right now. Your saved environment and conversations are retained.',
        );
      }
    } catch {
      if (inspection.current === request)
        setMessage(
          'Recovery could not be checked. Try again when the Sandbox status is available.',
        );
    } finally {
      clearTimeout(timeout);
      if (inspection.current === request) {
        inspection.current = undefined;
        setChecking(false);
      }
    }
  };
  const pending =
    command.intent !== undefined && !command.intent.receipt && !command.intent.refused;
  const unavailable =
    disabled ||
    stale ||
    !!command.storageError ||
    checking ||
    command.busy ||
    !client.recoverSandboxRetained;
  const consentIsCurrent =
    confirmation?.sandboxId === sandboxId &&
    confirmation.ownerId === ownerId &&
    confirmation.revision === failureRevision;
  if (!failed && !command.intent) return null;
  return (
    <section className="sandbox-page__band" aria-label="Retained recovery">
      <h2>Recover your Sandbox</h2>
      <p>
        Reopen your original conversations on the same saved environment, keeping its image, files
        and history.
      </p>
      {command.intent?.receipt ? (
        <p role="status">
          Recovery accepted. Follow the Sandbox progress, then return to your existing Agent.
        </p>
      ) : (
        <Button
          disabled={unavailable}
          onClick={
            pending
              ? command.retry
              : () => {
                  void inspect();
                }
          }
        >
          {command.busy
            ? 'Requesting recovery…'
            : checking
              ? 'Checking recovery…'
              : pending
                ? 'Retry recovery request'
                : 'Recover on the same image'}
        </Button>
      )}
      {command.storageError || message || command.intent?.message ? (
        <p role="status">{command.storageError ?? command.intent?.message ?? message}</p>
      ) : null}
      <Modal
        open={confirmation !== undefined}
        onClose={() => setConfirmation(undefined)}
        title="Recover on the same image"
        description="Your home, files, image and original conversations will be preserved."
        footer={
          <>
            <Button onClick={() => setConfirmation(undefined)}>Cancel</Button>
            <Button
              variant="primary"
              disabled={unavailable || !consentIsCurrent}
              onClick={() => {
                if (unavailable || !consentIsCurrent) return;
                command.start(confirmation.revision);
                setConfirmation(undefined);
              }}
            >
              Recover my Sandbox
            </Button>
          </>
        }
      >
        <p>
          This request reopens your existing conversations only when the saved environment can be
          verified. Uncertain work will not be replayed.
        </p>
        {confirmation !== undefined && !consentIsCurrent ? (
          <p>The Sandbox changed. Close this dialog and check recovery again.</p>
        ) : null}
      </Modal>
    </section>
  );
}
