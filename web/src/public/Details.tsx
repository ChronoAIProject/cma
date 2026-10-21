import type { TriggerPreview, LaunchTrustDisclosure } from '@chronoai/cma-sdk';
import { publicCopy } from '../vocabulary/needs';
import { ReferenceLink } from '../profiles/ReferenceLink';

type Disclosure = TriggerPreview['trust']['disclosures'][number] | LaunchTrustDisclosure;

export function SoftwareDetails({ disclosures }: { readonly disclosures: readonly Disclosure[] }) {
  return (
    <div className="public-software">
      {disclosures.map((item, index) => (
        <section key={index}>
          <h3>{'name' in item ? item.name : publicCopy.fullSoftware}</h3>
          {item.commands.map((command, at) => (
            <pre key={at}>
              <code>{command}</code>
            </pre>
          ))}
          {'environment_variables' in item && item.environment_variables.length > 0 ? (
            <>
              <h3>{publicCopy.environment}</h3>
              <dl>
                {item.environment_variables.map((entry) => (
                  <div key={entry.name}>
                    <dt>
                      <code>{entry.name}</code>
                    </dt>
                    <dd>
                      <code>{entry.value}</code>
                    </dd>
                  </div>
                ))}
              </dl>
            </>
          ) : null}
        </section>
      ))}
    </div>
  );
}

export function Details({
  preview,
  expanded = false,
}: {
  readonly preview: TriggerPreview;
  readonly expanded?: boolean;
}) {
  return (
    <details className="public-details" open={expanded || undefined}>
      <summary>{publicCopy.details}</summary>
      <dl>
        <div>
          <dt>{publicCopy.model}</dt>
          <dd>{preview.technical.model ?? publicCopy.defaults}</dd>
        </div>
        <div>
          <dt>{publicCopy.effort}</dt>
          <dd>{preview.technical.effort}</dd>
        </div>
        <div>
          <dt>{publicCopy.size}</dt>
          <dd>
            {preview.technical.size.cpu_millis / 1000} CPU /{' '}
            {preview.technical.size.memory_mib / 1024} GB
          </dd>
        </div>
        <div>
          <dt>{publicCopy.services}</dt>
          <dd>
            {preview.technical.services.length === 0 ? (
              publicCopy.none
            ) : (
              <ul>
                {preview.technical.services.map((service) => (
                  <li key={service}>
                    <ReferenceLink kind="service" id={service} />
                  </li>
                ))}
              </ul>
            )}
          </dd>
        </div>
      </dl>
      {preview.trust.disclosures.length > 0 ? (
        <>
          <p>{publicCopy.software}</p>
          <details>
            <summary>{publicCopy.fullSoftware}</summary>
            <SoftwareDetails disclosures={preview.trust.disclosures} />
          </details>
        </>
      ) : null}
      <h3>{publicCopy.firstMessage}</h3>
      <pre>{preview.first_message}</pre>
    </details>
  );
}
