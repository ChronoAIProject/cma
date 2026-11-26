import { LandingCharacter } from './landing/LandingCharacter';
import { Button } from './ui/Button';
import { Alert } from './ui/Alert';
import { Presence } from './ui/Presence';
import { IconNewChatOutline16 } from './ui/icons';
import type { useConsoleAgentAccess } from './resources/consoleAgentAccess';

/**
 * The centre column's plain states: a view that cannot open for this
 * caller, an address that matches nothing, and Agent Manager home. Kept out
 * of the console's routing so the routing reads as routing.
 */

function ConsoleActionError({ error }: { readonly error?: string }) {
  return (
    <Presence>
      {error === undefined ? null : (
        <p className="console__action-error" role="alert">
          {error}
        </p>
      )}
    </Presence>
  );
}

export function ConsoleNotices({
  error,
  fileDownload,
}: {
  readonly error?: string;
  readonly fileDownload: ReturnType<typeof useConsoleAgentAccess>['fileDownload'];
}) {
  return (
    <>
      <ConsoleActionError error={error} />
      {fileDownload.message ? (
        <Alert
          tone="error"
          actions={
            <Button size="sm" onClick={fileDownload.dismiss}>
              Dismiss
            </Button>
          }
        >
          {fileDownload.message}
        </Alert>
      ) : null}
    </>
  );
}

export function UnavailableView({
  title,
  reason,
}: {
  readonly title: string;
  /** Why this caller cannot open the view, in words they can act on. */
  readonly reason: string;
}) {
  return (
    <section className="console__empty">
      <h1>{title}</h1>
      <p>{reason}</p>
      <a href="/">Return to Agent Manager</a>
    </section>
  );
}

export function NotFoundView() {
  return (
    <section className="console__empty" aria-labelledby="not-found-title">
      <p className="console__eyebrow">404</p>
      <h1 id="not-found-title">Page not found</h1>
      <a href="/">Return to CMA</a>
    </section>
  );
}

export function WelcomeView({
  heading,
  description,
  onNewAgent,
}: {
  readonly heading: string;
  /** One sentence saying what an Agent is for, on Agent Manager home. */
  readonly description?: string;
  /** Present only when the caller can create an Agent right now. */
  readonly onNewAgent?: () => void;
}) {
  return (
    <section className="console__empty" aria-labelledby="empty-title">
      <div className="brand-home brand-home--compact">
        <LandingCharacter autoRotate />
      </div>
      <h1 id="empty-title">{heading}</h1>
      {description === undefined ? null : <p>{description}</p>}
      {onNewAgent === undefined ? null : (
        <div className="console__empty-actions">
          <Button variant="primary" icon={<IconNewChatOutline16 />} onClick={onNewAgent}>
            New agent
          </Button>
        </div>
      )}
    </section>
  );
}
