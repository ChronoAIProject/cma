import type { CreationProfile } from './client';

export function TrustConfirmation({
  profile,
  confirmed,
  onChange,
  disabled = false,
}: {
  readonly profile: CreationProfile;
  readonly confirmed: boolean;
  readonly onChange: (confirmed: boolean) => void;
  readonly disabled?: boolean;
}) {
  if (profile.disclosures.length === 0) return null;
  return (
    <section className="create-trust" aria-label="Profile commands and environment">
      {profile.disclosures.map((item, index) => (
        <div key={index}>
          <h3>{item.name}</h3>
          {item.commands.map((command, index) => (
            <pre key={index}>
              <code>{command}</code>
            </pre>
          ))}
          {item.environment.map((variable) => (
            <pre key={variable.name}>
              <code>
                {variable.name}={variable.value}
              </code>
            </pre>
          ))}
        </div>
      ))}
      {profile.requiresConfirmation ? (
        <label className="create-trust__confirm">
          <input
            type="checkbox"
            checked={confirmed}
            onChange={(event) => onChange(event.target.checked)}
            disabled={disabled}
          />
          I trust this profile
        </label>
      ) : null}
    </section>
  );
}
