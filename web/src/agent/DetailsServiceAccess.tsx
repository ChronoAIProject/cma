import type { MouseEvent } from 'react';
import type { AgentDetail } from '../api/types';
import { ReferenceLink } from '../profiles/ReferenceLink';
import { isCodexConnection } from '../profiles/modelProvider';
import { openSandboxTab, sandboxTabPath } from '../sandbox/sandboxTabRoute';
import { REQUIRED_AGENT_SCOPES } from './requiredScopes';

function plainClick(event: MouseEvent) {
  return !(event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey);
}

/**
 * The Agent's service access, read-only — `docs/Web_Style.md` §13.
 *
 * The NyxID key is the Sandbox's, shared by every Agent in it, so it is
 * edited in one place, the Sandbox page's Access tab. Editing it from one
 * Agent's drawer hid a Sandbox-wide change behind a single Agent.
 */
export function DetailsServiceAccess({
  detail,
  onOpenSandbox,
}: {
  readonly detail: AgentDetail;
  /** Client-side navigation to the Sandbox page; without it the link loads the page. */
  readonly onOpenSandbox?: () => void;
}) {
  const optional = [...new Set(detail.reach)]
    .filter(
      (id) => !REQUIRED_AGENT_SCOPES.some((required) => required === id) && !isCodexConnection(id),
    )
    .sort();
  const headingId = `${detail.id}-service-access`;
  return (
    <section className="agent-details__band agent-details__access" aria-labelledby={headingId}>
      <h2 id={headingId}>Service access</h2>
      <p>These services are shared by all Agents in this Sandbox.</p>
      <ul aria-label="Services this Agent can use">
        {REQUIRED_AGENT_SCOPES.map((id) => (
          <li key={id}>
            <span>{id}</span>
            <span className="agent-details__required">Required</span>
          </li>
        ))}
        {optional.map((id) => (
          <li key={id}>
            <ReferenceLink kind="service" id={id} />
          </li>
        ))}
      </ul>
      <a
        className="agent-details__link"
        href={sandboxTabPath(detail.sandbox.id, 'access')}
        onClick={(event) => {
          if (onOpenSandbox === undefined || !plainClick(event)) return;
          event.preventDefault();
          openSandboxTab(onOpenSandbox, detail.sandbox.id, 'access');
        }}
      >
        Manage on Sandbox
      </a>
    </section>
  );
}
