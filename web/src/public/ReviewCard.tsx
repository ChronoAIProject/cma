import { useEffect, useRef, useState } from 'react';
import {
  ApiError,
  type Launch,
  type ServiceConnectionMetadata,
  type StartLaunchRequest,
  type TriggerPreview,
} from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { needNames, needOrder, publicCopy } from '../vocabulary/needs';
import { refusalSentence } from '../vocabulary/errors';
import type { PublicClient } from './client';
import { isUnsettledCommand } from './admission';
import { ConnectAssistant } from './ConnectAssistant';
import { Details, SoftwareDetails } from './Details';

export function ReviewCard({
  client,
  record,
  preview,
  onChanged,
  onClose,
}: {
  readonly client: PublicClient;
  readonly record: Launch;
  readonly preview?: TriggerPreview;
  readonly onChanged: (value: Launch) => void;
  readonly onClose: () => void;
}) {
  const [confirmed, setConfirmed] = useState(false);
  const [connected, setConnected] = useState<readonly string[]>([]);
  const [metadata, setMetadata] = useState<readonly ServiceConnectionMetadata[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const active = useRef(false);
  const command = useRef<{ key: string; body: StartLaunchRequest } | undefined>(undefined);
  const [uncertain, setUncertain] = useState(false);
  const needs = [...record.review.needs].sort((a, b) => needOrder[a.type] - needOrder[b.type]);
  const trust = needs.find((need) => need.type === 'trust_confirmation');
  const ownerReview = needs.some((need) => need.type === 'owner_requires_review');
  const missing = needs.flatMap((need) => (need.type === 'connection' ? [need.service] : []));

  useEffect(() => {
    const controller = new AbortController();
    void client.requests.requirements(record.id, { signal: controller.signal }).then(
      (value) => {
        if (!controller.signal.aborted) setMetadata(value.services);
      },
      (cause: unknown) => {
        if (!controller.signal.aborted) setError(refusalSentence(cause));
      },
    );
    return () => controller.abort();
  }, [client, record.id, record.revision]);

  const start = async () => {
    if (active.current) return;
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
      const result = await client.requests.start(record.id, command.current.body, {
        idempotencyKey: command.current.key,
      });
      command.current = undefined;
      setUncertain(false);
      onChanged(result);
    } catch (cause) {
      const unknown = isUnsettledCommand(cause);
      setUncertain(unknown);
      if (!unknown) command.current = undefined;
      setError(refusalSentence(cause));
      if (
        cause instanceof ApiError &&
        [
          'launch_requirements_not_met',
          'trust_confirmation_mismatch',
          'launch_not_startable',
          'launch_changed',
        ].includes(cause.code ?? '')
      ) {
        if (cause.code === 'launch_requirements_not_met') setConnected([]);
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
  const satisfy = (service: string) => {
    const next = [...new Set([...connected, service])];
    setConnected(next);
    if (
      !ownerReview &&
      (trust === undefined || confirmed) &&
      missing.every((item) => next.includes(item))
    )
      void start();
  };
  const canStart = trust === undefined || confirmed;
  return (
    <Modal
      open
      title={publicCopy.review}
      onClose={onClose}
      dismissible={!busy}
      footer={
        <Button
          variant="primary"
          disabled={busy || (!uncertain && !canStart)}
          onClick={() => void start()}
        >
          {uncertain ? publicCopy.retry : publicCopy.continue}
        </Button>
      }
    >
      <div className="public-review">
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
              return connected.includes(need.service) ? null : (
                <ConnectAssistant
                  key={need.service}
                  client={client}
                  requestId={record.id}
                  service={
                    metadata.find((item) => item.service === need.service) ?? {
                      service: need.service,
                      state: 'unknown',
                    }
                  }
                  disabled={busy || uncertain}
                  onSatisfied={satisfy}
                  onRecheck={() => {
                    if (!ownerReview && (trust === undefined || confirmed)) void start();
                    else
                      void client.requests.requirements(record.id).then(
                        (value) => setMetadata(value.services),
                        (cause: unknown) => setError(refusalSentence(cause)),
                      );
                  }}
                />
              );
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
        {error === undefined ? null : (
          <p className="public-error" role="alert">
            {error}
          </p>
        )}
      </div>
    </Modal>
  );
}
