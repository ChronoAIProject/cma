import { lazy, Suspense, type ComponentProps } from 'react';
import type { DeliveryFlowCanvas } from './DeliveryFlowCanvas';

// The graph engine is only downloaded when someone opens Event Gateway.
const Canvas = lazy(() =>
  import('./DeliveryFlowCanvas').then((module) => ({ default: module.DeliveryFlowCanvas })),
);
export function DeliveryFlow(props: ComponentProps<typeof DeliveryFlowCanvas>) {
  return (
    <Suspense
      fallback={<div className="delivery-flow" role="status" aria-label="Loading delivery map" />}
    >
      <Canvas {...props} />
    </Suspense>
  );
}
