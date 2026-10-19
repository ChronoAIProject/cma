import { useEffect, useRef, useState } from 'react';
import type { WarmPoolList, WarmPoolView } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { IconRefreshOutline16 } from '../ui/icons';
import type { OperatorClient } from './client';
import { operatorFailure, type OperatorFailure } from './failure';
import { FailureNotice } from './records';
import { WarmPoolCard } from './WarmPoolCard';
import { WarmPoolCreate } from './WarmPoolCreate';
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
      {busy ? <p role="status">Loading warm pools…</p> : null}
      {data ? (
        <>
          {!data.available ? (
            <p className="notice notice--quiet" role="status">
              Warm pool capacity is currently unavailable. Saved targets will be applied when the
              runtime service is ready.
            </p>
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
          {current.length === 0 ? (
            <p className="operator-empty">
              No warm pools configured for the current runtime. New Agents can start without
              reserved capacity.
            </p>
          ) : null}
          <div className="warm-pool-list">
            {current.map((pool) => (
              <WarmPoolCard
                key={pool.id}
                client={client}
                pool={pool}
                currentDefault={
                  !!data.default_config_sha256 && pool.config_sha256 === data.default_config_sha256
                }
                onChanged={changed}
              />
            ))}
          </div>
          {previousNeedsAttention ? (
            <p className="operator-notice" role="status">
              Previous runtime pools still have spare capacity or need attention. Review them below;
              disable any remaining targets. Claimed Sandboxes keep running.
            </p>
          ) : null}
          {previous.length > 0 ? (
            <details className="warm-pool-create" open={previousNeedsAttention}>
              <summary>Previous runtime pools ({previous.length})</summary>
              <div className="warm-pool-list">
                <p className="operator-notice">
                  Previous runtime images are no longer prewarmed. These records remain available
                  for capacity and cleanup checks.
                </p>
                {previous.map((pool) => (
                  <WarmPoolCard
                    key={pool.id}
                    client={client}
                    pool={pool}
                    previousRuntime
                    onChanged={changed}
                  />
                ))}
              </div>
            </details>
          ) : null}
        </>
      ) : null}
    </section>
  );
}
