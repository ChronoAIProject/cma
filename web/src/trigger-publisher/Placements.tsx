import { Presence } from '../ui/Presence';
import { useEffect, useState } from 'react';
import type { TriggerDocument } from '@cma/client';
import type { CommandOptions } from '../api/client';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { InlineConfirm } from '../ui/InlineConfirm';
import { IconPlusOutline16, IconTrashOutline16 } from '../ui/icons';
import type { TriggerPublisherClient } from './client';
import { placementError } from './model';
import { usePublisherCommand } from './transaction';

export function Placements({
  client,
  document,
  disabled,
  saveDraft,
  onChange,
  onLock,
  onConflict,
}: {
  readonly client: TriggerPublisherClient;
  readonly document?: TriggerDocument;
  readonly disabled: boolean;
  readonly saveDraft: (options: CommandOptions) => Promise<TriggerDocument>;
  readonly onChange: (document: TriggerDocument) => void;
  readonly onLock: (locked: boolean) => void;
  readonly onConflict: () => void;
}) {
  const [label, setLabel] = useState('');
  const [source, setSource] = useState('');
  const [validation, setValidation] = useState<string>();
  const [retiring, setRetiring] = useState<string>();
  const command = usePublisherCommand<TriggerDocument>(
    (next) => {
      onChange(next);
      setLabel('');
      setSource('');
      setRetiring(undefined);
    },
    (failure) => {
      if (failure.conflict) onConflict();
    },
  );
  useEffect(() => {
    onLock(command.locked);
    return () => onLock(false);
  }, [command.locked, onLock]);
  const placements =
    document?.placements.filter((placement) => placement.retired_at === undefined) ?? [];
  const locked = disabled || command.locked;
  const add = () => {
    const error = placementError(label, source);
    setValidation(error);
    if (error !== undefined) return;
    const body = { label, source_url: source };
    let saved: TriggerDocument | undefined;
    void command.submit(async (options) => {
      saved ??= await saveDraft({ ...options, idempotencyKey: `${options.idempotencyKey}:draft` });
      const placement = await client.triggers.createPlacement(saved.id, body, {
        ...options,
        idempotencyKey: `${options.idempotencyKey}:placement`,
      });
      return { ...saved, placements: [...saved.placements, placement] };
    });
  };
  const retire = (placementId: string) => {
    if (document === undefined) return;
    void command.submit(async (options) => {
      const retired = await client.triggers.retirePlacement(document.id, placementId, options);
      return {
        ...document,
        placements: document.placements.map((placement) =>
          placement.id === placementId ? retired : placement,
        ),
      };
    });
  };
  return (
    <section aria-label="Source URLs">
      <ul className="trigger-placements">
        {placements.map((placement) => (
          <li key={placement.id}>
            <div>
              <strong>{placement.label || 'Unlabeled placement'}</strong>
              <p>{placement.source_url}</p>
              <span className="trigger-caption">{placement.id}</span>
            </div>
            {retiring === placement.id ? (
              <InlineConfirm
                question={`Remove ${placement.label || placement.source_url}? Existing launch records remain.`}
                confirmLabel="Remove placement"
                busy={locked}
                cancelDisabled={command.locked}
                onCancel={() => setRetiring(undefined)}
                onConfirm={() => retire(placement.id)}
              />
            ) : (
              <Button
                icon={<IconTrashOutline16 />}
                aria-label={`Remove ${placement.label || placement.source_url}`}
                title="Remove placement"
                disabled={locked}
                onClick={() => setRetiring(placement.id)}
              />
            )}
          </li>
        ))}
      </ul>
      {placements.length === 0 ? <p>No source URLs</p> : null}
      <div className="trigger-field-pair">
        <div className="trigger-field">
          <label htmlFor="placement-label">Source label</label>
          <Input
            id="placement-label"
            value={label}
            disabled={locked}
            onChange={(event) => setLabel(event.target.value)}
          />
        </div>
        <div className="trigger-field">
          <label htmlFor="placement-source">Source URL</label>
          <Input
            id="placement-source"
            type="url"
            value={source}
            disabled={locked}
            onChange={(event) => setSource(event.target.value)}
          />
        </div>
      </div>
      <Presence>
        {validation === undefined ? null : (
          <p role="alert" className="trigger-error">
            {validation}
          </p>
        )}
      </Presence>
      <Button
        icon={<IconPlusOutline16 />}
        disabled={locked || placements.length >= 32}
        onClick={add}
      >
        Add source URL
      </Button>
      <Presence>
        {command.failure === undefined || command.failure.conflict ? null : (
          <div role="alert">
            <p>{command.failure.message}</p>
            {command.failure.uncertain ? (
              <Button disabled={command.pending} onClick={command.retry}>
                Retry original request
              </Button>
            ) : null}
          </div>
        )}
      </Presence>
      <p className="trigger-caption">
        Source URLs provide attribution. They do not authenticate visitors or grant access to the
        publisher's resources.
      </p>
    </section>
  );
}
