import { Chip } from '../ui/Chip';
import type { PlatformService } from './types';

/**
 * Which of the owner's platform services a session may reach.
 *
 * A session is given a credential minted for it alone, and this is what that
 * credential is allowed to reach. Ticking nothing does not produce a session
 * that can reach nothing: what the deployment mandates is added whatever this
 * says, which is why those rows are shown fixed rather than hidden — a person
 * deciding what a session may touch should see the whole answer.
 *
 * Deliberately not searchable. An account has services in the tens, not the
 * hundreds like the skill registry, and a search box over a short list is
 * chrome that makes a small decision look like a large one.
 */

interface ServicePickerProps {
  readonly services: readonly PlatformService[];
  readonly selected: readonly string[];
  readonly mandatory: readonly string[];
  readonly onChange: (selected: readonly string[]) => void;
  /** Why the list could not be fetched, if it could not be. */
  readonly error?: string | undefined;
}

export function ServicePicker({
  services,
  selected,
  mandatory,
  onChange,
  error,
}: ServicePickerProps) {
  function toggle(id: string, checked: boolean): void {
    if (mandatory.includes(id)) {
      // Unreachable through the UI, which disables these. Enforced anyway:
      // the control plane adds them back, and a form that disagrees with what
      // was saved is worse than one that never let it change.
      return;
    }
    onChange(checked ? [...selected, id] : selected.filter((s) => s !== id));
  }

  if (error !== undefined) {
    return (
      <p className="field__error" role="alert">
        {error} Your profile still saves — a session reaches what this deployment mandates until the
        list can be read.
      </p>
    );
  }

  return (
    <div className="picker">
      {services.length === 0 ? (
        <p className="empty">No services are available to you yet.</p>
      ) : (
        services.map((service) => {
          const required = mandatory.includes(service.id);
          return (
            <label className="picker__row" key={service.id}>
              <input
                type="checkbox"
                checked={required || selected.includes(service.id)}
                disabled={required}
                onChange={(event) => {
                  toggle(service.id, event.target.checked);
                }}
              />
              <span className="picker__text">
                <span className="picker__name">
                  {service.name}
                  {required ? <Chip inline>always</Chip> : null}
                </span>
                {service.description === undefined ? null : (
                  <span className="picker__description">{service.description}</span>
                )}
              </span>
            </label>
          );
        })
      )}
    </div>
  );
}
