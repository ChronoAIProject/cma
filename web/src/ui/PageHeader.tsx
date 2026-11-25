import type { MouseEvent, ReactNode, Ref } from 'react';
import './PageHeader.css';

/** One step of the breadcrumb. Without an href it is plain text. */
export interface Crumb {
  readonly label: string;
  readonly href?: string;
  /** Client-side navigation for a plain click; a modified click stays the browser's. */
  readonly onSelect?: () => void;
}

function plainClick(event: MouseEvent) {
  return !(event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey);
}

/**
 * The one header every centre view starts with — the shared style contracts
 *
 * Every view used to invent its own heading, so the same question — where am
 * I, what state is this in, what can I do — was answered in a different place
 * on each page. The header answers it in one place: a breadcrumb of real
 * links, the title with its status and short facts beside it, an optional
 * line of description, and the actions on the right, where a view puts at
 * most one primary, one secondary and a More actions menu.
 */
export function PageHeader({
  breadcrumb = [],
  title,
  headingRef,
  headingId,
  badge,
  meta,
  description,
  actions,
  children,
  placement,
  wrapIdentity = false,
}: {
  readonly breadcrumb?: readonly Crumb[];
  readonly title: ReactNode;
  readonly headingRef?: Ref<HTMLHeadingElement>;
  readonly headingId?: string;
  /** The resource's status, beside the title. */
  readonly badge?: ReactNode;
  /** Short facts beside the status, such as a branch. */
  readonly meta?: ReactNode;
  readonly description?: ReactNode;
  readonly actions?: ReactNode;
  /** Anything that belongs to the header but not its rows, such as a notice. */
  readonly children?: ReactNode;
  /** Extra class for the owning surface's placement only. */
  readonly placement?: string;
  /** Show the complete title and breadcrumb across multiple lines. */
  readonly wrapIdentity?: boolean;
}) {
  return (
    <header
      className={['page-header', placement ?? ''].filter(Boolean).join(' ')}
      data-wrap-identity={wrapIdentity || undefined}
    >
      {breadcrumb.length === 0 ? null : (
        <nav className="page-header__crumbs" aria-label="Breadcrumb">
          <ol>
            {breadcrumb.map((crumb, index) => {
              const last = index === breadcrumb.length - 1;
              return (
                <li key={`${String(index)}:${crumb.label}`}>
                  {crumb.href === undefined || last ? (
                    <span aria-current={last ? 'page' : undefined} title={crumb.label}>
                      {crumb.label}
                    </span>
                  ) : (
                    <a
                      href={crumb.href}
                      title={crumb.label}
                      onClick={(event) => {
                        if (!plainClick(event) || crumb.onSelect === undefined) return;
                        event.preventDefault();
                        crumb.onSelect();
                      }}
                    >
                      {crumb.label}
                    </a>
                  )}
                </li>
              );
            })}
          </ol>
        </nav>
      )}
      <div className="page-header__row">
        <div className="page-header__heading">
          <div className="page-header__titleline">
            <h1 className="page-header__title" ref={headingRef} id={headingId} tabIndex={-1}>
              {title}
            </h1>
            {badge === undefined ? null : <span className="page-header__badge">{badge}</span>}
            {meta === undefined ? null : <span className="page-header__meta">{meta}</span>}
          </div>
          {description === undefined || description === null ? null : (
            <div className="page-header__description">{description}</div>
          )}
        </div>
        {actions === undefined || actions === null ? null : (
          <div className="page-header__actions">{actions}</div>
        )}
      </div>
      {children}
    </header>
  );
}
