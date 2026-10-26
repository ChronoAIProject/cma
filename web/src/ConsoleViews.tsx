import { LandingCharacter } from './landing/LandingCharacter';
import { Button } from './ui/Button';
import { IconNewChatOutline16 } from './ui/icons';

/**
 * The centre column's plain states: a view that cannot open for this
 * caller, an address that matches nothing, and Agent Manager home. Kept out
 * of the console's routing so the routing reads as routing.
 */

export function UnavailableView({ title }: { readonly title: string }) {
  return (
    <section className="console__empty">
      <h1>{title}</h1>
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
  onNewAgent,
}: {
  readonly heading: string;
  /** Present only when the caller can create an Agent right now. */
  readonly onNewAgent?: () => void;
}) {
  return (
    <section className="console__empty" aria-labelledby="empty-title">
      <div className="brand-home brand-home--compact">
        <LandingCharacter autoRotate />
      </div>
      <h1 id="empty-title">{heading}</h1>
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
