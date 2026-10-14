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
          <WarmPoolCreate
            client={client}
            image={data.default_image}
            cpuMillis={data.default_cpu_millis}
            memoryMib={data.default_memory_mib}
            onChanged={changed}
          />
          {data.items.length === 0 ? (
            <p className="operator-empty">
              No warm pools configured. New Agents can start without reserved capacity.
            </p>
          ) : null}
          <div className="warm-pool-list">
            {data.items.map((pool) => (
              <WarmPoolCard key={pool.id} client={client} pool={pool} onChanged={changed} />
            ))}
          </div>
        </>
      ) : null}
    </section>
  );
}
