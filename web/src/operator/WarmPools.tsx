import { useEffect, useRef, useState } from 'react';
import type { WarmPoolList, WarmPoolView } from '@cma/client';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { SkeletonRows } from '../ui/Skeleton';
import { IconRefreshOutline16 } from '../ui/icons';
import type { OperatorClient } from './client';
import { operatorFailure, type OperatorFailure } from './failure';
import { FailureNotice } from './records';
import { WarmPoolCreate } from './WarmPoolCreate';
import { WarmPoolRuntime } from './WarmPoolRuntime';
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
              ? current.items.map((item) =>
                  item.id === pool.id
                    ? {
                        ...item,
                        ...pool,
                        current_runtime: pool.current_runtime ?? item.current_runtime,
                      }
                    : item,
                )
              : [...current.items, pool],
          },
    );
  };
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
          <p className="operator-notice">
            Set a target to 0 to disable spare capacity. Agents that have already claimed a Sandbox
            keep running.
          </p>
          <WarmPoolRuntime
            title="Standard runtime"
            currentLabel="Current runtime pools"
            image={data.default_image}
            configSha={data.default_config_sha256}
            claimPoolId={data.default_claim_pool_id}
            capacityPoolId={data.default_capacity_pool_id}
            showMatchingSize
            available={data.available}
            pools={data.items.filter((pool) => pool.machine !== true)}
            empty="No warm pools configured for the current runtime."
            versions={data.runtime_versions}
            client={client}
            onChanged={changed}
          >
            <WarmPoolCreate
              client={client}
              image={data.default_image}
              cpuMillis={data.default_cpu_millis}
              memoryMib={data.default_memory_mib}
              onChanged={changed}
            />
          </WarmPoolRuntime>
          <WarmPoolRuntime
            title="Shared desktop runtime"
            currentLabel="Shared desktop pool"
            image={data.machine_image}
            configSha={data.machine_config_sha256}
            claimPoolId={data.machine_claim_pool_id}
            capacityPoolId={data.machine_capacity_pool_id}
            available={data.available}
            pools={data.items.filter((pool) => pool.machine === true)}
            empty="No shared desktop pool is recorded for this image yet."
            versions={data.runtime_versions}
            client={client}
            onChanged={changed}
          />
        </>
      ) : null}
    </section>
  );
}
