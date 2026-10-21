import type { Ref } from 'react';
import { BrandLoading } from '../landing/LandingCharacter';
import { Button } from '../ui/Button';
import { PageHeader } from '../ui/PageHeader';

/** The Agent surface before its Agent has loaded. The heading takes focus. */
export function AgentOpening({ headingRef }: { readonly headingRef: Ref<HTMLHeadingElement> }) {
  return (
    <section className="agent-surface agent-surface--loading" aria-busy="true">
      <PageHeader placement="agent-header" title="Agent" headingRef={headingRef} />
      <div className="agent-loading">
        <BrandLoading label="Opening your agent" />
      </div>
    </section>
  );
}

/** The Agent surface when its Agent could not be read, with one way to retry. */
export function AgentUnavailable({
  message,
  headingRef,
  onRetry,
}: {
  readonly message: string;
  readonly headingRef: Ref<HTMLHeadingElement>;
  readonly onRetry: () => void;
}) {
  return (
    <section className="agent-surface agent-surface--error" role="alert">
      <div>
        <p className="eyebrow">Agent</p>
        <h1 ref={headingRef} tabIndex={-1}>
          Agent unavailable
        </h1>
        <p>{message}</p>
        <Button variant="outline" onClick={onRetry}>
          Try again
        </Button>
      </div>
    </section>
  );
}
