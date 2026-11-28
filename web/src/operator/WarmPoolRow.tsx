import { Fragment, useRef, useState } from 'react';
import type { WarmPoolView } from '@cma/client';
import { Badge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { ValidatedNumberInput } from './ValidatedNumberInput';
import { TableActions, TableCell, TableNote } from '../ui/Table';
import type { StatusTone } from '../vocabulary/status';
import type { OperatorClient } from './client';
import { operatorFailure, type OperatorFailure } from './failure';
import { FailureNotice } from './records';
import { validPoolNumber } from './WarmPoolFields';
import { WarmPoolImage } from './WarmPoolImage';

const STATUS = {
  pending: { label: 'Pending', tone: 'active' },
  ready: { label: 'Ready', tone: 'neutral' },
  disabled: { label: 'Disabled', tone: 'muted' },
  unavailable: { label: 'Unavailable', tone: 'warning' },
} as const satisfies Record<
  WarmPoolView['status'],
  { readonly label: string; readonly tone: StatusTone }
>;

export function warmPoolColumns(previousRuntime: boolean) {
  return [
    { header: 'Size' },
    { header: 'Status' },
    { header: 'Desired', align: 'end' as const },
    { header: 'Ready', align: 'end' as const },
    { header: 'Provider target', align: 'end' as const },
    { header: 'Pending', align: 'end' as const },
    // A previous runtime is never prewarmed again, so it has no target to edit.
    ...(previousRuntime ? [] : [{ header: 'Target' }]),
  ];
}

/**
 * One warm pool as a table row, with its own note row beneath it.
 *
 * Every write names the revision it read. A conflict or an unconfirmed
 * outcome keeps the operator's draft and blocks further writes until the
 * current target has been read, so a change is never repeated blindly.
 */
export function WarmPoolRow({
  client,
  pool,
  previousRuntime = false,
  currentDefault = false,
  defaultConfiguration = false,
  matchingSize = false,
  runtimeVersion,
  onChanged,
}: {
  readonly client: OperatorClient;
  readonly pool: WarmPoolView;
  readonly previousRuntime?: boolean;
  readonly currentDefault?: boolean;
  readonly defaultConfiguration?: boolean;
  readonly matchingSize?: boolean;
  readonly runtimeVersion?: string | undefined;
  readonly onChanged: (pool: WarmPoolView) => void;
}) {
  const [target, setTarget] = useState(String(pool.target_replicas));
  const [revision, setRevision] = useState(pool.revision);
  const [busy, setBusy] = useState(false);
  const [review, setReview] = useState(false);
  const [failure, setFailure] = useState<OperatorFailure>();
  const [saved, setSaved] = useState(false);
  const [disableRequested, setDisableRequested] = useState(false);
  const disableConfirmation = useRef(false);
  const submitting = useRef(false);
  const cancelDisable = () => {
    disableConfirmation.current = false;
    setDisableRequested(false);
  };
  const needsReview = review || revision !== pool.revision;
  async function save(replicas: number) {
    if (submitting.current || needsReview || (previousRuntime && replicas !== 0)) return;
    submitting.current = true;
    setBusy(true);
    setFailure(undefined);
    setSaved(false);
    try {
      const updated = await client.admin.resizeWarmPool(pool.id, {
        expected_revision: revision,
        target_replicas: replicas,
      });
      setTarget(String(updated.target_replicas));
      setRevision(updated.revision);
      setSaved(true);
      onChanged(updated);
    } catch (error) {
      const problem = operatorFailure(error);
      setFailure(problem);
      setReview(problem.conflict || problem.uncertain);
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  }
  async function reread() {
    if (submitting.current) return;
    submitting.current = true;
    setBusy(true);
    try {
      const result = await client.admin.listWarmPools();
      const current = result.items.find((item) => item.id === pool.id);
      if (current === undefined) throw new Error('Pool missing');
      setTarget(String(current.target_replicas));
      setRevision(current.revision);
      setReview(false);
      setFailure(undefined);
      onChanged(current);
    } catch (error) {
      setFailure(operatorFailure(error));
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  }
  const ready = pool.observed ? pool.ready_replicas : null;
  const pending = ready === null ? null : Math.max(0, pool.target_replicas - ready);
  const label = `${String(pool.cpu_millis)} millicores / ${String(pool.memory_mib)} MiB`;
  const status = STATUS[pool.status];
  const note =
    !pool.observed || pool.error !== null || failure !== undefined || needsReview || saved;
  return (
    <Fragment>
      <tr aria-label={`Warm pool ${label}`}>
        <TableCell>
          <span className="warm-pool-size">{label}</span>
          {previousRuntime ? <WarmPoolImage image={pool.image} version={runtimeVersion} /> : null}
          {currentDefault ? (
            <span className="warm-pool-tag">Used for new default-size Agents</span>
          ) : null}
          {!currentDefault && defaultConfiguration ? (
            <span className="warm-pool-tag">Default configuration</span>
          ) : null}
          {matchingSize ? <span className="warm-pool-tag">For matching size</span> : null}
          {pool.config_sha256 ? (
            <details className="warm-pool-config">
              <summary>Pool configuration</summary>
              <code className="warm-pool-reference">{pool.config_sha256}</code>
            </details>
          ) : null}
        </TableCell>
        <TableCell>
          <Badge tone={status.tone}>{status.label}</Badge>
        </TableCell>
        <TableCell align="end">{pool.target_replicas}</TableCell>
        <TableCell align="end">{ready ?? 'Unknown'}</TableCell>
        <TableCell align="end">{pool.provider_target_replicas ?? 'Unknown'}</TableCell>
        <TableCell align="end">{pending ?? 'Unknown'}</TableCell>
        {previousRuntime ? null : (
          <TableCell>
            <form
              className="warm-pool-target"
              onSubmit={(event) => {
                event.preventDefault();
                if (validPoolNumber(target)) void save(Number(target));
              }}
            >
              <ValidatedNumberInput
                aria-label="Ready Sandbox target"
                required
                min={0}
                max={1000}
                step={1}
                value={target}
                disabled={busy || needsReview || disableRequested}
                onChange={(event) => {
                  setTarget(event.target.value);
                  setSaved(false);
                }}
              />
              <Button
                type="submit"
                size="sm"
                disabled={
                  busy ||
                  needsReview ||
                  disableRequested ||
                  !validPoolNumber(target) ||
                  Number(target) === pool.target_replicas
                }
              >
                {busy ? 'Updating…' : 'Update target'}
              </Button>
            </form>
          </TableCell>
        )}
        <TableActions>
          <Button
            size="sm"
            disabled={busy || needsReview || disableRequested || pool.target_replicas === 0}
            onClick={() => {
              disableConfirmation.current = true;
              setDisableRequested(true);
            }}
          >
            Disable pool
          </Button>
        </TableActions>
      </tr>
      {note ? (
        <TableNote>
          <div className="warm-pool-note">
            {!pool.observed ? <p>Waiting for the controller to confirm current capacity.</p> : null}
            {pool.error ? (
              <p className="operator-failure" role="alert">
                {pool.error}
              </p>
            ) : null}
            <FailureNotice failure={failure} />
            {needsReview ? (
              <div className="operator-actions">
                <p role="status">
                  Your draft is retained. Read the current target before making another change.
                </p>
                <Button size="sm" disabled={busy} onClick={() => void reread()}>
                  Read current target
                </Button>
              </div>
            ) : null}
            {saved ? <p role="status">Target saved.</p> : null}
          </div>
        </TableNote>
      ) : null}
      <Modal
        open={disableRequested}
        title="Disable warm pool"
        onClose={cancelDisable}
        footer={
          <div className="operator-actions">
            <Button onClick={cancelDisable}>Cancel</Button>
            <Button
              variant="primary"
              disabled={busy || needsReview || pool.target_replicas === 0}
              onClick={() => {
                // The closing dialog can retain its old handler during its exit animation.
                if (
                  !disableConfirmation.current ||
                  busy ||
                  needsReview ||
                  pool.target_replicas === 0
                )
                  return;
                cancelDisable();
                void save(0);
              }}
            >
              Confirm disable
            </Button>
          </div>
        }
      >
        <p>
          Disable pool {pool.id} ({label}), reducing its target from {pool.target_replicas} to 0?
          This removes spare capacity for new work. Claimed Sandboxes keep running.
        </p>
        {needsReview ? <p>Read the current target before confirming this change.</p> : null}
      </Modal>
    </Fragment>
  );
}
