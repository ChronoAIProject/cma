import { useState } from 'react';
import type { AgentDetail, SandboxDetail, SandboxProfileSettings } from '../api/types';
import { ScopeSettings } from '../agent/ScopeSettings';
import { loadScopeUpdate } from '../agent/scopeUpdateStorage';
import { Button } from '../ui/Button';
import { ReferenceLink } from '../profiles/ReferenceLink';
import { isCodexConnection } from '../profiles/modelProvider';
import type { SandboxContents } from '../resources/retainedOwnerViews';

/**
 * The services every Agent in this Sandbox may call through its shared NyxID
 * key, as the public Sandbox record reports them.
 *
 * Effective reach comes from the Sandbox's service list, with an Agent's
 * lineage as the older-server fallback. Requested profile scope is shown
 * separately and never stands in for reach: unconfirmed access says so.
 */
export function SandboxServiceReach({
  sandbox,
  contents,
  settings,
}: {
  readonly sandbox: SandboxDetail;
  readonly contents?: SandboxContents;
  readonly settings?: SandboxProfileSettings;
}) {
  const reach = (sandbox.nyxidServices ?? contents?.agents[0]?.reach)?.filter(
    (id) => !isCodexConnection(id),
  );
  return (
    <dl>
      <div>
        <dt>NyxID services</dt>
        <dd>
          {reach === undefined ? (
            'Access not yet confirmed'
          ) : reach.length === 0 ? (
            'None'
          ) : (
            <ul className="sandbox-page__references">
              {reach.map((id) => (
                <li key={id}>
                  <ReferenceLink kind="service" id={id} />
                </li>
              ))}
            </ul>
          )}
          {reach === undefined && settings !== undefined && settings.nyxidServices.length > 0 ? (
            <>
              <p>Requested services</p>
              <ul className="sandbox-page__references">
                {settings.nyxidServices
                  .filter((id) => !isCodexConnection(id))
                  .map((id) => (
                    <li key={id}>
                      <ReferenceLink kind="service" id={id} />
                    </li>
                  ))}
              </ul>
            </>
          ) : null}
        </dd>
      </div>
    </dl>
  );
}

const RUNNING = ['starting', 'ready', 'working', 'waiting_approval', 'paused'];

/**
 * The Agent whose service-access command carries a scope change.
 *
 * The key belongs to the Sandbox, but the public command is addressed to one
 * of its Agents, which is notified when the key changes. An Agent holding a
 * retained unsent or unconfirmed update comes first, so a retry keeps its
 * original key and request; then a running Agent; then any Agent.
 */
function scopeAgent(agents: readonly AgentDetail[], owner: string): AgentDetail | undefined {
  const live = agents.filter((agent) => agent.status !== 'deleted');
  return (
    live.find((agent) => {
      const saved = loadScopeUpdate(owner, agent.id);
      return saved.intent !== undefined || saved.error !== undefined;
    }) ??
    live.find((agent) => RUNNING.includes(agent.status)) ??
    live[0]
  );
}

/** The Access tab — `docs/Web_Style.md` §17. */
export function SandboxAccess({
  sandbox,
  contents,
  settings,
  owner,
  active,
  onRetry,
}: {
  readonly sandbox: SandboxDetail;
  readonly contents?: SandboxContents;
  readonly settings?: SandboxProfileSettings;
  /** The signed-in viewer, when they own this Sandbox. */
  readonly owner?: string;
  /** Whether the tab is on screen; the editor checks connections only then. */
  readonly active: boolean;
  readonly onRetry: () => void;
}) {
  const candidate =
    owner === undefined || contents === undefined ? undefined : scopeAgent(contents.agents, owner);
  // Keep the first choice while it remains, so a refresh of the children
  // cannot swap the editor away from a selection that has not been saved.
  const [chosen, setChosen] = useState<string>();
  const kept = contents?.agents.find((agent) => agent.id === chosen && agent.status !== 'deleted');
  if (kept === undefined && candidate !== undefined && chosen !== candidate.id)
    setChosen(candidate.id);
  const agent = kept ?? candidate;
  const hasAgents = contents?.workspaces.some((workspace) =>
    workspace.agents.some((row) => row.status !== 'deleted'),
  );
  return (
    <section className="sandbox-page__band" aria-label="Service access">
      <h2>Service access</h2>
      {owner !== undefined && agent !== undefined ? (
        <div data-scope-editor="">
          <ScopeSettings
            key={`scope:${owner}:${agent.id}`}
            detail={agent}
            owner={owner}
            active={active}
          />
        </div>
      ) : (
        <>
          <p>These services are shared by all Agents in this Sandbox.</p>
          <SandboxServiceReach sandbox={sandbox} contents={contents} settings={settings} />
          {owner === undefined || contents === undefined ? null : contents.failed ? (
            <p>
              Service access could not be loaded.{' '}
              <Button size="sm" onClick={onRetry}>
                Try again
              </Button>
            </p>
          ) : hasAgents ? null : (
            <p>Service access can be changed once this Sandbox has an Agent.</p>
          )}
        </>
      )}
    </section>
  );
}
