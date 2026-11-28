import { useRef, useState } from 'react';
import type { AutoPausePreview, PauseIdleNow } from '@cma/client';
import { Button } from '../ui/Button';
import type { OperatorClient } from './client';
import { operatorFailure, type OperatorFailure } from './failure';
import { FailureNotice, stateWord } from './records';

export function AutoPauseControls({
  client,
  disabled,
}: {
  readonly client: OperatorClient;
  readonly disabled: boolean;
}) {
  const [preview, setPreview] = useState<AutoPausePreview>();
  const [paused, setPaused] = useState<PauseIdleNow>();
  const [failure, setFailure] = useState<OperatorFailure>();
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState<{ cursor?: string }>();
  const pending = useRef(false);
  const run = async (pause: boolean, cursor?: string) => {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    setFailure(undefined);
    try {
      if (pause) {
        setPaused(await client.admin.pauseIdleNow(true, { limit: 25, cursor }));
        setConfirm(undefined);
      } else {
        setPreview(await client.admin.previewAutoPause({ limit: 25, cursor }));
      }
    } catch (error) {
      setFailure(operatorFailure(error));
    } finally {
      pending.current = false;
      setBusy(false);
    }
  };
  return (
    <div>
      <p>Use saved settings to check up to 25 Sandboxes per batch.</p>
      <div className="operator-actions">
        <Button disabled={disabled || busy} onClick={() => void run(false)}>
          Preview idle Sandboxes
        </Button>
        {preview?.next == null ? null : (
          <Button
            disabled={disabled || busy}
            onClick={() => void run(false, preview.next ?? undefined)}
          >
            {(preview.failed ?? 0) > 0 ? 'Retry preview batch' : 'Preview next batch'}
          </Button>
        )}
        <Button disabled={disabled || busy} onClick={() => setConfirm({})}>
          Pause idle now
        </Button>
        {paused?.next == null ? null : (
          <Button
            disabled={disabled || busy}
            onClick={() => setConfirm({ cursor: paused.next ?? undefined })}
          >
            {(paused.failed ?? 0) > 0 ? 'Review unconfirmed batch' : 'Pause next batch'}
          </Button>
        )}
      </div>
      {confirm === undefined ? null : (
        <div>
          <p>
            Pause eligible Sandboxes in this batch? This bypasses enablement grace only. Active
            work, the idle threshold, and Always on exceptions are rechecked.
          </p>
          <div className="operator-actions">
            <Button disabled={disabled || busy} onClick={() => void run(true, confirm.cursor)}>
              Confirm pause batch
            </Button>
            <Button disabled={busy} onClick={() => setConfirm(undefined)}>
              Cancel
            </Button>
          </div>
        </div>
      )}
      {busy ? <p role="status">Checking this batch…</p> : null}
      {preview === undefined ? null : (
        <p role="status">
          {preview.enabled === false
            ? 'Auto pause is disabled in saved settings.'
            : `${preview.candidates} eligible Sandboxes in this preview batch.${(preview.failed ?? 0) > 0 ? ' Some checks could not be confirmed.' : ''}`}
        </p>
      )}
      {preview?.refusals?.length ? (
        <ul aria-label="Sandbox preview refusals">
          {preview.refusals.map(({ sandbox_id, reason }) => (
            <li key={sandbox_id}>
              <code>{sandbox_id}</code> · {stateWord(reason)}
            </li>
          ))}
        </ul>
      ) : null}
      {paused === undefined ? null : (
        <p role="status">
          {paused.enabled === false
            ? 'Auto pause is disabled; no pauses were scheduled.'
            : `${paused.scheduled} scheduled, ${paused.already_satisfied} already scheduled, ${paused.skipped} skipped in this batch.${(paused.failed ?? 0) > 0 ? ' Some outcomes could not be confirmed. Review before continuing.' : ''}`}
        </p>
      )}
      <FailureNotice failure={failure} />
    </div>
  );
}
