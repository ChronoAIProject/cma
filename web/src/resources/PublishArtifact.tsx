import { useEffect, useRef, useState } from 'react';
import { ApiError, ValidationError, type PublishAgentArtifactRequest } from '@chronoai/cma-sdk';
import type { ResourceViewsClient } from './client';
import { Button } from '../ui/Button';

const supported = [
  'text/plain',
  'text/markdown',
  'application/json',
  'application/pdf',
  'application/octet-stream',
  'image/png',
  'image/jpeg',
  'image/webp',
];
type RetainedPublication = { body: PublishAgentArtifactRequest; key: string };
export class ArtifactPublicationRecovery {
  #pending: RetainedPublication | undefined;
  get pending() {
    return this.#pending;
  }
  retain(value: RetainedPublication | undefined) {
    this.#pending = value;
  }
}
interface Props {
  readonly enabled?: boolean;
  readonly client: ResourceViewsClient;
  readonly id: string;
  readonly onPublished: () => void;
  readonly onAccessChanged: () => void;
  readonly recovery?: ArtifactPublicationRecovery;
}
export function PublishArtifact(props: Props) {
  const [scope, setScope] = useState({
    client: props.client,
    id: props.id,
    recovery: props.recovery,
    revision: 0,
  });
  if (scope.client !== props.client || scope.id !== props.id || scope.recovery !== props.recovery)
    setScope({
      client: props.client,
      id: props.id,
      recovery: props.recovery,
      revision: scope.revision + 1,
    });
  return <PublicationForm key={scope.revision} {...props} />;
}
function PublicationForm({
  client,
  id,
  onPublished,
  onAccessChanged,
  recovery,
  enabled = true,
}: Props) {
  const [file, setFile] = useState<File>();
  const [confirmed, setConfirmed] = useState(recovery?.pending !== undefined);
  const [busy, setBusy] = useState(false);
  const [retrying, setRetrying] = useState(recovery?.pending !== undefined);
  const [message, setMessage] = useState<string>();
  const request = useRef<AbortController | undefined>(undefined);
  const retained = useRef<{ body: PublishAgentArtifactRequest; key: string } | undefined>(
    recovery?.pending,
  );
  useEffect(() => () => request.current?.abort(), [client, id]);
  const run = async () => {
    if (
      !enabled ||
      !confirmed ||
      (file === undefined && retained.current === undefined) ||
      request.current !== undefined
    )
      return;
    const pending = new AbortController();
    request.current = pending;
    setBusy(true);
    setMessage(undefined);
    try {
      if (retained.current === undefined) {
        if (file === undefined || file.size === 0 || file.size > 1024 * 1024)
          throw new Error('Choose a file between 1 byte and 1 MiB.');
        const media = file.type || 'application/octet-stream';
        if (!supported.includes(media))
          throw new Error('This file type is not supported for publication.');
        const bytes = new Uint8Array(await file.arrayBuffer());
        pending.signal.throwIfAborted();
        let binary = '';
        for (let offset = 0; offset < bytes.length; offset += 8192)
          binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
        retained.current = {
          body: {
            name: file.name,
            media_type: media as PublishAgentArtifactRequest['media_type'],
            content_base64: btoa(binary),
          },
          key: crypto.randomUUID(),
        };
        setRetrying(true);
        recovery?.retain(retained.current);
      }
      const current = await client.resourceAccess.effective('agent', id, {
        signal: pending.signal,
      });
      if (!current.capabilities.includes('manage_sharing')) {
        onAccessChanged();
        throw new Error('Current access does not allow publishing artifacts.');
      }
      pending.signal.throwIfAborted();
      await client.agents.publishArtifact(id, retained.current.body, {
        idempotencyKey: retained.current.key,
        signal: pending.signal,
      });
      pending.signal.throwIfAborted();
      retained.current = undefined;
      recovery?.retain(undefined);
      setRetrying(false);
      setFile(undefined);
      setConfirmed(false);
      setMessage('Artifact published.');
      onPublished();
    } catch (error) {
      if (!pending.signal.aborted) {
        if (
          error instanceof ValidationError ||
          (error instanceof ApiError &&
            error.status === 400 &&
            error.code === 'invalid_artifact_request')
        ) {
          retained.current = undefined;
          recovery?.retain(undefined);
          setRetrying(false);
        }
        setMessage(
          error instanceof Error
            ? error.message
            : 'Publication could not be confirmed. Retry to check the same request.',
        );
        if (error instanceof ApiError && [401, 403, 404, 409].includes(error.status))
          onAccessChanged();
      }
    } finally {
      if (request.current === pending) request.current = undefined;
      if (!pending.signal.aborted) setBusy(false);
    }
  };
  return (
    <div className="agent-artifacts">
      <p>
        Publish a selected file for this Agent’s current and future readers. Published content is
        retained with the Agent. Previously downloaded copies cannot be recalled.
      </p>
      <label className="agent-artifacts__file">
        Artifact file
        <input
          type="file"
          disabled={busy || retrying}
          onChange={(event) => {
            setFile(event.target.files?.[0]);
            setConfirmed(false);
            setMessage(undefined);
          }}
        />
      </label>
      <label className="agent-artifacts__approval">
        <input
          type="checkbox"
          checked={confirmed}
          disabled={busy}
          onChange={(event) => setConfirmed(event.target.checked)}
        />
        <span>I approve sharing this file with everyone who can read this Agent.</span>
      </label>
      <Button
        disabled={!enabled || busy || !confirmed || (file === undefined && !retrying)}
        onClick={() => void run()}
      >
        {busy
          ? 'Publishing artifact'
          : !retrying
            ? 'Publish artifact'
            : 'Retry artifact publication'}
      </Button>
      {message === undefined ? null : <p role="status">{message}</p>}
    </div>
  );
}
