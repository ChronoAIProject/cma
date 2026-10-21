import { useEffect, useRef, useState } from 'react';
import type { WarmPoolList, WarmPoolView } from '@chronoai/cma-sdk';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { SkeletonRows } from '../ui/Skeleton';
import { Table } from '../ui/Table';
import { IconRefreshOutline16 } from '../ui/icons';
import type { OperatorClient } from './client';
import { operatorFailure, type OperatorFailure } from './failure';
import { FailureNotice } from './records';
import { WarmPoolCreate } from './WarmPoolCreate';
import { WarmPoolRow, warmPoolColumns } from './WarmPoolRow';
import './WarmPools.css';

export function WarmPools({ client }: { readonly client: OperatorClient }) {
  const [data, setData] = useState<WarmPoolList>();
  const [failure, setFailure] = useState<OperatorFailure>();
  const [busy, setBusy] = useState(true);
  const [reload, setReload] = useState(0);
  const mutation = useRef(0);
  useEffect(() => {
    const controller = new AbortController();
    const before = mutation.current;
    client.admin.listWarmPools({ signal: controller.signal }).then(
      (value) => {
        if (!controller.signal.aborted) {
          if (before === mutation.current) setData(value);
          else setReload((value) => value + 1);
          setFailure(undefined);
          setBusy(false);
        }
      },
      (error: unknown) => {
        if (!controller.signal.aborted) {
          setFailure(operatorFailure(error));
          setBusy(false);
        }
      },
    );
    return () => controller.abort();
  }, [client, reload]);
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (document.visibilityState === 'visible') setReload((value) => value + 1);
    }, 10000);
    return () => window.clearInterval(timer);
  }, []);
  const changed = (pool: WarmPoolView) => {
    mutation.current += 1;
    setData((current) =>
      current === undefined
        ? current
        : {
            ...current,
            items: current.items.some((item) => item.id === pool.id)
              ? current.items.map((item) => (item.id === pool.id ? pool : item))
              : [...current.items, pool],
          },
    );
  };
  const current = data?.items.filter((pool) => pool.image === data.default_image) ?? [];
  const previous = data?.items.filter((pool) => pool.image !== data.default_image) ?? [];
  const previousNeedsAttention = previous.some(
    (pool) =>
      pool.target_replicas > 0 ||
      (pool.ready_replicas ?? 0) > 0 ||
      !pool.observed ||
      pool.error !== null,
  );
  return (
    <section className="warm-pools" aria-label="Warm pools">
      <header className="warm-pool-header">
        <div>
          <h2>Warm pools</h2>
          <p className="operator-notice">
            Prewarm runtime images with a fixed CPU and memory allocation to shorten Agent startup.
          </p>
        </div>
        <Button
          disabled={busy}
          icon={<IconRefreshOutline16 />}
          onClick={() => {
            setBusy(true);
            setReload((value) => value + 1);
          }}
        >
          Refresh pools
        </Button>
      </header>
      <FailureNotice failure={failure} />
      {/* A refresh keeps the tables in place; only a first read reserves their rows. */}
      {busy && data === undefined ? <SkeletonRows label="Loading warm pools" /> : null}
      {data ? (
        <>
          {!data.available ? (
            <Alert tone="info">
              Warm pool capacity is currently unavailable. Saved targets will be applied when the
              runtime service is ready.
            </Alert>
          ) : null}
          <div className="warm-pool-image">
            <span>Current runtime image</span>
            <code>{data.default_image || 'No runtime image configured'}</code>
          </div>
          <WarmPoolCreate
            client={client}
            image={data.default_image}
            cpuMillis={data.default_cpu_millis}
            memoryMib={data.default_memory_mib}
            onChanged={changed}
          />
          <div className="warm-pool-list">
            <p className="operator-notice">
              Set a target to 0 to disable spare capacity. Agents that have already claimed a
              Sandbox keep running.
            </p>
            <Table
              label="Current runtime pools"
              columns={warmPoolColumns(false)}
              actions="Actions"
              empty="No warm pools configured for the current runtime. New Agents can start without reserved capacity."
            >
              {current.map((pool) => (
                <WarmPoolRow
                  key={pool.id}
                  client={client}
                  pool={pool}
                  currentDefault={
                    !!data.default_config_sha256 &&
                    pool.config_sha256 === data.default_config_sha256
                  }
                  onChanged={changed}
                />
              ))}
            </Table>
          </div>
          {previousNeedsAttention ? (
            <Alert tone="warning">
              Previous runtime pools still have spare capacity or need attention. Review them below;
              disable any remaining targets. Claimed Sandboxes keep running.
            </Alert>
          ) : null}
          {previous.length > 0 ? (
            <details className="warm-pool-create" open={previousNeedsAttention}>
              <summary>Previous runtime pools ({previous.length})</summary>
              <div className="warm-pool-list">
                <p className="operator-notice">
                  Previous runtime images are no longer prewarmed. Only disabling is available for a
                  previous runtime; claimed Sandboxes keep running. These records remain available
                  for capacity and cleanup checks.
                </p>
                <Table
                  label="Previous runtime pools"
                  columns={warmPoolColumns(true)}
                  actions="Actions"
                >
                  {previous.map((pool) => (
                    <WarmPoolRow
                      key={pool.id}
                      client={client}
                      pool={pool}
                      previousRuntime
                      onChanged={changed}
                    />
                  ))}
                </Table>
              </div>
            </details>
          ) : null}
        </>
      ) : null}
    </section>
  );
}
