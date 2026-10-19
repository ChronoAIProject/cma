import { useRef, useState } from 'react';
import type { WarmPoolView } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import type { OperatorClient } from './client';
import { operatorFailure, type OperatorFailure } from './failure';
import { FailureNotice } from './records';
import { validPoolNumber, WarmPoolNumber } from './WarmPoolFields';

export function WarmPoolCard({
  client,
  pool,
  previousRuntime = false,
  currentDefault = false,
  onChanged,
}: {
  readonly client: OperatorClient;
  readonly pool: WarmPoolView;
  readonly previousRuntime?: boolean;
  readonly currentDefault?: boolean;
  readonly onChanged: (pool: WarmPoolView) => void;
}) {
  const [target, setTarget] = useState(String(pool.target_replicas));
  const [revision, setRevision] = useState(pool.revision);
  const [busy, setBusy] = useState(false);
  const [review, setReview] = useState(false);
  const [failure, setFailure] = useState<OperatorFailure>();
  const [saved, setSaved] = useState(false);
  const submitting = useRef(false);
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
  const label = `${pool.cpu_millis} millicores / ${pool.memory_mib} MiB`;
  return (
    <article className="warm-pool-card card" aria-label={`Warm pool ${label}`}>
      <header className="warm-pool-header">
        <h3>{label}</h3>
        <span className="operator-state">{pool.status}</span>
      </header>
      <code className="warm-pool-reference">{pool.image}</code>
      {currentDefault ? <p className="operator-notice">Current default configuration</p> : null}
      {pool.config_sha256 ? (
        <details>
          <summary>Pool configuration</summary>
          <code className="warm-pool-reference">{pool.config_sha256}</code>
        </details>
      ) : null}
      <dl className="warm-pool-counts">
        <div>
          <dt>Desired</dt>
          <dd>{pool.target_replicas}</dd>
        </div>
        <div>
          <dt>Ready</dt>
          <dd>{ready ?? 'Unknown'}</dd>
        </div>
        <div>
          <dt>Pending</dt>
          <dd>{pending ?? 'Unknown'}</dd>
        </div>
      </dl>
      {!pool.observed ? (
        <p className="operator-notice">Waiting for the controller to confirm current capacity.</p>
      ) : null}
      {pool.error ? (
        <p className="operator-failure" role="alert">
          {pool.error}
        </p>
      ) : null}
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (!previousRuntime && validPoolNumber(target)) void save(Number(target));
        }}
      >
        {previousRuntime ? null : (
          <WarmPoolNumber
            label="Ready Sandbox target"
            value={target}
            onChange={(value) => {
              setTarget(value);
              setSaved(false);
            }}
            disabled={busy || needsReview}
          />
        )}
        <p className="operator-notice">
          {previousRuntime
            ? 'Only disabling is available for a previous runtime. Claimed Sandboxes keep running.'
            : 'Set to 0 to disable spare capacity. Agents that have already claimed a Sandbox keep running.'}
        </p>
        <FailureNotice failure={failure} />
        {needsReview ? (
          <p role="status">
            Your draft is retained. Read the current target before making another change.
          </p>
        ) : null}
        <div className="operator-actions">
          {previousRuntime ? null : (
            <Button
              type="submit"
              disabled={
                busy ||
                needsReview ||
                !validPoolNumber(target) ||
                Number(target) === pool.target_replicas
              }
            >
              {busy ? 'Updating…' : 'Update target'}
            </Button>
          )}
          <Button
            disabled={busy || needsReview || pool.target_replicas === 0}
            onClick={() => void save(0)}
          >
            Disable pool
          </Button>
          {needsReview ? (
            <Button disabled={busy} onClick={() => void reread()}>
              Read current target
            </Button>
          ) : null}
          {saved ? <p role="status">Target saved.</p> : null}
        </div>
      </form>
    </article>
  );
}
