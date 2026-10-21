import type { SandboxDetail, SandboxProfileSettings } from '../api/types';
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

/** The Access tab — `docs/Web_Style.md` §17. */
export function SandboxAccess({
  sandbox,
  contents,
  settings,
}: {
  readonly sandbox: SandboxDetail;
  readonly contents?: SandboxContents;
  readonly settings?: SandboxProfileSettings;
}) {
  return (
    <section className="sandbox-page__band" aria-label="Service access">
      <h2>Service access</h2>
      <p>These services are shared by all Agents in this Sandbox.</p>
      <SandboxServiceReach sandbox={sandbox} contents={contents} settings={settings} />
    </section>
  );
}
