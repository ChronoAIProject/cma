import type { ReactNode } from 'react';
import type { WarmPoolList, WarmPoolView } from '@cma/client';
import { Alert } from '../ui/Alert';
import { Table } from '../ui/Table';
import type { OperatorClient } from './client';
import { WarmPoolImage } from './WarmPoolImage';
import { WarmPoolRow, warmPoolColumns } from './WarmPoolRow';

export function WarmPoolRuntime({
  title,
  currentLabel,
  image,
  configSha,
  claimPoolId,
  capacityPoolId,
  showMatchingSize = false,
  available,
  pools,
  empty,
  versions,
  client,
  onChanged,
  children,
}: {
  readonly title: string;
  readonly currentLabel: string;
  readonly image: string | null | undefined;
  readonly configSha: string | null | undefined;
  readonly claimPoolId: string | null | undefined;
  readonly capacityPoolId: string | null | undefined;
  readonly showMatchingSize?: boolean;
  readonly available: boolean;
  readonly pools: readonly WarmPoolView[];
  readonly empty: string;
  readonly versions: WarmPoolList['runtime_versions'];
  readonly client: OperatorClient;
  readonly onChanged: (pool: WarmPoolView) => void;
  readonly children?: ReactNode;
}) {
  const isCurrent = (pool: WarmPoolView) =>
    pool.current_runtime ?? (pool.image === image && !pool.retirement_status);
  const current = pools.filter(isCurrent);
  const previous = pools.filter((pool) => !isCurrent(pool));
  const claimPool =
    available && claimPoolId ? current.find((pool) => pool.id === claimPoolId) : undefined;
  const capacityPool = available ? current.find((pool) => pool.id === capacityPoolId) : undefined;
  const spareDisabled = capacityPool?.target_replicas === 0;
  const ready = capacityPool?.observed ? capacityPool.ready_replicas : null;
  const previousNeedsAttention = previous.some(
    (pool) =>
      pool.target_replicas > 0 ||
      (pool.ready_replicas ?? 0) > 0 ||
      !pool.observed ||
      pool.error !== null,
  );
  const versionFor = (image: string) =>
    versions?.find((release) => release.image === image)?.version;
  return (
    <section className="warm-pool-runtime warm-pool-list" aria-label={title}>
      <h3>{title}</h3>
      {image ? (
        <WarmPoolImage image={image} version={versionFor(image)} />
      ) : (
        <p className="operator-notice">No runtime image configured.</p>
      )}
      {image && spareDisabled ? (
        <p className="operator-notice">Spare capacity disabled for the default pool.</p>
      ) : null}
      {image && !spareDisabled && ready === 0 ? (
        <Alert tone="warning" title="No ready spare">
          New default-size Agents may start without a prewarmed Sandbox.
        </Alert>
      ) : null}
      {image && !spareDisabled && ready === null ? (
        <p className="operator-notice">Ready capacity unknown for new default-size Agents.</p>
      ) : null}
      {children}
      {image ? (
        <Table
          label={currentLabel}
          columns={warmPoolColumns(false)}
          actions="Actions"
          empty={empty}
        >
          {current.map((pool) => (
            <WarmPoolRow
              key={pool.id}
              client={client}
              pool={pool}
              currentDefault={pool === claimPool}
              defaultConfiguration={Boolean(configSha && pool.config_sha256 === configSha)}
              matchingSize={
                showMatchingSize &&
                pool.current_runtime === true &&
                pool.config_sha256 !== configSha
              }
              onChanged={onChanged}
            />
          ))}
        </Table>
      ) : null}
      {previousNeedsAttention ? (
        <Alert tone="warning">
          Previous runtime pools still have spare capacity or need attention. Review them below.
          Claimed Sandboxes keep running.
        </Alert>
      ) : null}
      {previous.length > 0 ? (
        <details className="warm-pool-create" open={previousNeedsAttention}>
          <summary>Previous runtime pools ({previous.length})</summary>
          <div className="warm-pool-list">
            <Table label="Previous runtime pools" columns={warmPoolColumns(true)} actions="Actions">
              {previous.map((pool) => (
                <WarmPoolRow
                  key={pool.id}
                  client={client}
                  pool={pool}
                  previousRuntime
                  runtimeVersion={versionFor(pool.image)}
                  onChanged={onChanged}
                />
              ))}
            </Table>
          </div>
        </details>
      ) : null}
    </section>
  );
}
