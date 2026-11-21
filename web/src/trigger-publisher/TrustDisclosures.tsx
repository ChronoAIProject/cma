import type { SelectedProfiles } from './model';

export function TrustDisclosures({ profiles }: { readonly profiles: SelectedProfiles }) {
  return (
    <>
      {Object.values(profiles).flatMap((profile) => {
        if (profile.kind === 'agent' || !profile.public) return [];
        const commands =
          profile.kind === 'sandbox'
            ? profile.content.software_install_commands
            : profile.content.setup_commands;
        const environment = profile.kind === 'sandbox' ? profile.content.environment_variables : [];
        if (commands.length === 0 && environment.length === 0) return [];
        return [
          <details className="trigger-trust" key={`${profile.kind}:${profile.id}`}>
            <summary>{profile.name}: commands and environment</summary>
            <p className="trigger-caption">
              Revision {profile.revision}; {profile.digest}
            </p>
            {commands.map((command, index) => (
              <pre key={index}>
                <code>{command}</code>
              </pre>
            ))}
            {environment.length === 0 ? null : (
              <dl>
                {environment.map((variable) => (
                  <div key={variable.name}>
                    <dt>{variable.name}</dt>
                    <dd>{variable.value}</dd>
                  </div>
                ))}
              </dl>
            )}
          </details>,
        ];
      })}
    </>
  );
}
