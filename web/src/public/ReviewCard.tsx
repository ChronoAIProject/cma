import { Presence } from '../ui/Presence';
import { useRef, useState } from 'react';
import { ApiError, type Launch, type TriggerPreview } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import {
  checkCredentialRecoveryOwner,
  CredentialAccountError,
  credentialSetupMessage,
  needsSandboxCredentialSetup,
} from '../auth/credentialSetup';
import { SandboxCredentialSetup } from '../auth/SandboxCredentialSetup';
import { routePath } from '../routing/routes';
import { Modal } from '../ui/Modal';
import { needNames, needOrder, publicCopy } from '../vocabulary/needs';
import { refusalSentence } from '../vocabulary/errors';
import { connectionCopy } from '../vocabulary/connect';
import type { PublicClient } from './client';
import { isUnsettledCommand } from './admission';
import { ConnectionWizard } from '../connections/ConnectionWizard';
import type { ConnectionTarget } from '../connections/client';
import { Details, SoftwareDetails } from './Details';
import {
  clearDraftRecovery,
  readDraftRecovery,
  writeDraftRecovery,
  type DraftCredentialRecovery,
  type DraftStartCommand,
} from './draftRecovery';

export function ReviewCard({
  client,
  record,
  preview,
  expectedSubject,
  onChanged,
  onClose,
}: {
  readonly client: PublicClient;
  readonly record: Launch;
  readonly preview?: TriggerPreview;
  readonly expectedSubject?: string;
  readonly onChanged: (value: Launch) => void;
  readonly onClose: () => void;
}) {
  const [recovery, setRecovery] = useState(() => readDraftRecovery(record.id));
  const [confirmed, setConfirmed] = useState(
    recovery?.confirmedDigest !== undefined &&
      record.review.needs.some(
        (need) =>
          need.type === 'trust_confirmation' && need.commands_digest === recovery.confirmedDigest,
      ),
  );
  const [connectionsReady, setConnectionsReady] = useState(false);
  const [connectionRevision, setConnectionRevision] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | undefined>(
    recovery === undefined ? undefined : credentialSetupMessage,
  );
  const active = useRef(false);
  const command = useRef<DraftStartCommand | undefined>(recovery?.command);
  const [uncertain, setUncertain] = useState(recovery?.command !== undefined);
  const [needsSignIn, setNeedsSignIn] = useState(recovery !== undefined);
  const [credentialOwner, setCredentialOwner] = useState(recovery?.owner ?? expectedSubject);
  const explicitRetryRequired = useRef(recovery !== undefined);
  const credentialReturnTo = routePath({ kind: 'public_trigger', publicId: record.public_id });
  const needs = [...record.review.needs].sort((a, b) => needOrder[a.type] - needOrder[b.type]);
  const trust = needs.find((need) => need.type === 'trust_confirmation');
  const ownerReview = needs.some((need) => need.type === 'owner_requires_review');
  const missing = needs.flatMap((need) => (need.type === 'connection' ? [need.service] : []));
  const connectionsOnly = missing.length > 0 && missing.length === needs.length;

  const start = async (explicit = false) => {
    if (active.current || needsSignIn || (!explicit && explicitRetryRequired.current)) return;
    if (explicit) explicitRetryRequired.current = false;
    active.current = true;
    command.current ??= {
      key: crypto.randomUUID(),
      body: {
        trust_confirmed_digest: confirmed ? trust?.commands_digest : undefined,
        acknowledgements: [],
      },
    };
    setBusy(true);
    setError(undefined);
    try {
      if (recovery !== undefined)
        writeDraftRecovery(record.id, { ...recovery, command: command.current });
      await checkCredentialRecoveryOwner(recovery?.owner);
      const result = await client.requests.start(record.id, command.current.body, {
        idempotencyKey: command.current.key,
      });
      command.current = undefined;
      clearDraftRecovery(record.id);
      setUncertain(false);
      onChanged(result);
    } catch (cause) {
      const unknown = isUnsettledCommand(cause);
      setUncertain(unknown);
      const required =
        needsSandboxCredentialSetup(cause) || cause instanceof CredentialAccountError;
      setNeedsSignIn(required);
      if (required || recovery !== undefined) explicitRetryRequired.current = true;
      if (!unknown) command.current = undefined;
      setError(refusalSentence(cause));
      if ((required || recovery !== undefined) && credentialOwner !== undefined) {
        const retained: DraftCredentialRecovery = {
          owner: recovery?.owner ?? credentialOwner,
          confirmedDigest: confirmed ? trust?.commands_digest : undefined,
          command: command.current,
        };
        setRecovery(retained);
        try {
          writeDraftRecovery(record.id, retained);
        } catch {
          setError(publicCopy.storageUnavailable);
        }
      }
      if (
        cause instanceof ApiError &&
        [
          'launch_requirements_not_met',
          'trust_confirmation_mismatch',
          'launch_not_startable',
          'launch_changed',
        ].includes(cause.code ?? '')
      ) {
        if (cause.code === 'launch_requirements_not_met') {
          setConnectionsReady(false);
          setConnectionRevision((value) => value + 1);
        }
        try {
          onChanged(await client.requests.read(record.id));
        } catch {
          /* Keep the last observed review when its refresh fails. */
        }
      }
    } finally {
      active.current = false;
      setBusy(false);
    }
  };
  const satisfy = () => {
    setConnectionsReady(true);
    if (!ownerReview && (trust === undefined || confirmed)) void start();
  };
  const canStart = connectionsReady && (trust === undefined || confirmed);
  const connectionTarget: ConnectionTarget = { type: 'launch', id: record.id };
  return (
    <Modal
      open
      title={connectionsOnly ? connectionCopy.required : publicCopy.review}
      onClose={onClose}
      dismissible={!busy}
      footer={
        <Button
          variant="primary"
          disabled={busy || needsSignIn || (!uncertain && !canStart)}
          onClick={() => void start(true)}
        >
          {uncertain
            ? publicCopy.retry
            : connectionsOnly
              ? connectionCopy.verifyAll
              : publicCopy.continue}
        </Button>
      }
    >
      <div className="public-review">
        <ConnectionWizard
          key={`${record.id}:${connectionRevision}`}
          target={connectionTarget}
          onReady={satisfy}
          onCancel={onClose}
        />
        {needs.map((need, index) => {
          switch (need.type) {
            case 'trust_confirmation':
              return (
                <section key={index}>
                  <h3>{needNames[need.type]}</h3>
                  <SoftwareDetails disclosures={record.trust.disclosures} />
                  <label className="public-confirm">
                    <input
                      type="checkbox"
                      checked={confirmed}
                      disabled={busy || uncertain}
                      onChange={(event) => setConfirmed(event.target.checked)}
                    />
                    {publicCopy.confirmation}
                  </label>
                </section>
              );
            case 'connection':
              return null;
            case 'owner_requires_review':
              return (
                <section key={index}>
                  <h3>{needNames[need.type]}</h3>
                  <p>{publicCopy.reviewDetails}</p>
                  {preview === undefined ? (
                    <SoftwareDetails disclosures={record.trust.disclosures} />
                  ) : (
                    <Details preview={preview} expanded />
                  )}
                </section>
              );
            case 'repeat_choice':
              return <p key={index}>{needNames[need.type]}</p>;
          }
        })}
        <Presence>
          {error === undefined ? null : (
            <p className="public-error" role="alert">
              {error}
            </p>
          )}
        </Presence>
        {needsSignIn ? (
          <SandboxCredentialSetup
            returnTo={credentialReturnTo}
            expectedSubject={credentialOwner}
            onChecked={(subject) => {
              setCredentialOwner((original) => original ?? subject);
              setNeedsSignIn(false);
              setError(undefined);
            }}
          />
        ) : null}
      </div>
    </Modal>
  );
}
