import { useMemo, useState } from 'react';
import type { CatalogSkill } from './types';

/**
 * The multi-select over the skill registry.
 *
 * A selection list per docs/Web_Style.md section 6: checkbox rows, selection
 * signalled by the checkbox alone, a search field once the list is long enough
 * to hunt through.
 *
 * Mandatory skills are shown checked and disabled with a `required` chip
 * beside them, rather than hidden. Hiding them would leave a user wondering
 * why a session loads something they never chose, and disabling without saying
 * why reads as a bug.
 */

/** Past this many rows, scanning beats scrolling. */
const SEARCH_THRESHOLD = 10;

interface SkillPickerProps {
  readonly skills: readonly CatalogSkill[];
  readonly selected: readonly string[];
  readonly mandatory: readonly string[];
  readonly onChange: (selected: readonly string[]) => void;
  /** Why the registry could not be listed, when it could not be. */
  readonly error?: string | undefined;
}

export function SkillPicker({ skills, selected, mandatory, onChange, error }: SkillPickerProps) {
  const [query, setQuery] = useState('');

  const matching = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (needle.length === 0) {
      return skills;
    }
    return skills.filter(
      (s) => s.name.toLowerCase().includes(needle) || s.description.toLowerCase().includes(needle),
    );
  }, [skills, query]);

  function toggle(id: string, checked: boolean): void {
    if (mandatory.includes(id)) {
      // Unreachable through the UI, which disables these. Enforced anyway:
      // the server would put it back, and a list that disagrees with what was
      // saved is worse than one that never let it change.
      return;
    }
    onChange(checked ? [...selected, id] : selected.filter((s) => s !== id));
  }

  if (error !== undefined) {
    return (
      <p className="field__error" role="alert">
        {error} Your profile still saves — skills can be added once the registry answers.
      </p>
    );
  }

  return (
    <>
      {skills.length > SEARCH_THRESHOLD ? (
        <input
          type="search"
          className="field__input"
          aria-label="Search skills"
          placeholder="Search skills"
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
          }}
        />
      ) : null}

      <div className="picker">
        {matching.length === 0 ? (
          <p className="empty">
            {skills.length === 0
              ? 'No skills are available to you yet.'
              : 'Nothing matches that search.'}
          </p>
        ) : (
          matching.map((skill) => {
            const required = mandatory.includes(skill.id);
            return (
              <label className="picker__row" key={skill.id}>
                <input
                  type="checkbox"
                  checked={required || selected.includes(skill.id)}
                  disabled={required}
                  onChange={(event) => {
                    toggle(skill.id, event.target.checked);
                  }}
                />
                <span className="picker__text">
                  <span className="picker__name">
                    {skill.name}
                    {required ? <span className="chip chip--neutral">required</span> : null}
                    {skill.access === undefined || required ? null : (
                      <span className="picker__access">{skill.access}</span>
                    )}
                  </span>
                  <span className="picker__description">{skill.description}</span>
                </span>
              </label>
            );
          })
        )}
      </div>
    </>
  );
}
