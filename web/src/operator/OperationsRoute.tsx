import { Suspense } from 'react';
import { Operations } from '../ConsolePages';
import { UnavailableView } from '../ConsoleViews';
import type { Capability } from '../auth/types';
import type { OperatorClient } from './client';
import { canOpenOperations } from './capabilities';
import { type UsageReportOwner } from '../usage/metrics/reportOwner';

export function OperationsRoute({
  operator,
  capabilities,
  principal,
  bindingDiagnostics = false,
}: {
  readonly operator?: OperatorClient;
  readonly capabilities: readonly Capability[];
  readonly principal: UsageReportOwner;
  readonly bindingDiagnostics?: boolean;
}) {
  if (operator === undefined || !canOpenOperations(capabilities))
    return (
      <UnavailableView
        title="Operations unavailable"
        reason="Operations is for CMA operators. Ask your CMA administrator if you need access."
      />
    );
  return (
    <Suspense
      fallback={
        <section className="console__empty" role="status">
          Loading Operations
        </section>
      }
    >
      <Operations
        client={operator}
        capabilities={capabilities}
        principal={principal}
        bindingDiagnostics={bindingDiagnostics}
      />
    </Suspense>
  );
}
