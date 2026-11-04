import type { Ref } from 'react';
import { SkeletonRows } from '../ui/Skeleton';
import { Button } from '../ui/Button';
import { PageHeader } from '../ui/PageHeader';
import { Alert } from '../ui/Alert';
import type { AgentController } from './controller';

export function AgentCommandNotices({
  controller,
  unavailable,
  starting,
}: {
  readonly controller: AgentController;
  readonly unavailable: boolean;
  readonly starting: boolean;
}) {
  return (
    <>
      {controller.pendingMessage === undefined && !controller.pendingControl ? null : (
        <Alert
          tone="warning"
          actions={
            <Button
              size="sm"
              disabled={
                unavailable ||
                controller.pending !== undefined ||
                (starting && !controller.inputAvailability.allowed)
              }
              onClick={() => void controller.recoverPending?.()}
            >
              Recover pending request
            </Button>
          }
        >
          {controller.pendingMessage !== undefined
            ? 'Your message is awaiting confirmation. Recover it before sending another message.'
            : 'Your response action is awaiting confirmation.'}
        </Alert>
      )}
      {controller.controlNotice === undefined ? null : (
        <Alert tone="info">{controller.controlNotice}</Alert>
      )}
    </>
  );
}

/** The Agent surface before its Agent has loaded. The heading takes focus. */
export function AgentOpening({ headingRef }: { readonly headingRef: Ref<HTMLHeadingElement> }) {
  return (
    <section className="agent-surface agent-surface--loading" aria-busy="true">
      <PageHeader placement="agent-header" title="Agent" headingRef={headingRef} />
      <div className="agent-loading">
        <SkeletonRows rows={4} label="Opening your agent" />
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
