import { lazy, Suspense } from 'react';
import type { BridgePageProps } from './BridgePage';

const BridgePage = lazy(() =>
  import('./BridgePage').then((module) => ({ default: module.BridgePage })),
);

export function BridgeEntry(
  props: Omit<BridgePageProps, 'client'> & { readonly client?: BridgePageProps['client'] },
) {
  if (props.client === undefined)
    return (
      <section className="console__empty">
        <h1>Bridges unavailable</h1>
      </section>
    );
  return (
    <Suspense
      fallback={
        <section className="console__empty" role="status">
          Loading bridges
        </section>
      }
    >
      <BridgePage {...props} client={props.client} />
    </Suspense>
  );
}
