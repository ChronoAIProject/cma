import { useId, useState } from 'react';
import { SkillPicker } from './SkillPicker';
import { Chip } from '../ui/Chip';
import type { CatalogSkill, MandatoryEntries, ProfileDraft, ProfileVariable } from './types';
import { isValidKey } from './types';

/**
 * The profile form: a name, variables, software, and skills.
 *
 * Controlled by its parent so the dialog's footer owns the save action — a
 * form whose submit button lives outside it is the price of the dialog pattern
 * in docs/Web_Style.md, and the alternative is two competing action rows.
 */

interface ProfileEditorProps {
  readonly draft: ProfileDraft;
  readonly onChange: (draft: ProfileDraft) => void;
  readonly skills: readonly CatalogSkill[];
  readonly skillsError?: string | undefined;
  readonly mandatory: MandatoryEntries;
  /** What went wrong with the last attempt to save, if anything. */
  readonly error?: string | undefined;
}

export function ProfileEditor({
  draft,
  onChange,
  skills,
  skillsError,
  mandatory,
  error,
}: ProfileEditorProps) {
  const nameId = useId();
  const packagesId = useId();
  /*
   * Which values are currently shown. A variable's value is where a user puts
   * a token, so it is masked until they deliberately reveal it — per row, and
   * never remembered beyond this form.
   */
  const [revealed, setRevealed] = useState<readonly number[]>([]);

  /*
   * A mandated package appears in the field only when the owner pinned a
   * version of it — that pin is theirs to edit. The plain mandated entry is
   * implied and stays out of the way, because a name a user cannot remove is
   * noise in a field they are typing into.
   */
  const implied = draft.packages.filter(
    (p) => mandatory.packages.includes(p.name) && p.version === undefined,
  );
  const chosen = draft.packages.filter((p) => !implied.includes(p));

  function setVariable(index: number, next: ProfileVariable): void {
    onChange({
      ...draft,
      variables: draft.variables.map((v, i) => (i === index ? next : v)),
    });
  }

  return (
    <>
      {error === undefined ? null : (
        <p className="field__error" role="alert">
          {error}
        </p>
      )}

      <div className="field">
        <label className="field__label" htmlFor={nameId}>
          Name
        </label>
        <input
          id={nameId}
          className="field__input"
          value={draft.name}
          onChange={(event) => {
            onChange({ ...draft, name: event.target.value });
          }}
        />
      </div>

      <div className="field">
        <span className="field__label">Environment variables</span>
        {draft.variables.length === 0 ? (
          <p className="empty">No variables yet. Add one to set it in every session.</p>
        ) : (
          <div className="pairs">
            {draft.variables.map((variable, index) => (
              // Index-keyed deliberately: a variable has no identity until it
              // is named, and keying on a name that is being typed remounts
              // the field on every keystroke and loses the caret.
              <div className="pair" key={index}>
                <input
                  className="field__input"
                  aria-label={`Variable ${index + 1} name`}
                  aria-invalid={variable.key.length > 0 && !isValidKey(variable.key)}
                  value={variable.key}
                  onChange={(event) => {
                    setVariable(index, { ...variable, key: event.target.value });
                  }}
                />
                <input
                  className="field__input"
                  type={revealed.includes(index) ? 'text' : 'password'}
                  aria-label={`Variable ${index + 1} value`}
                  value={variable.value}
                  onChange={(event) => {
                    setVariable(index, { ...variable, value: event.target.value });
                  }}
                />
                <button
                  type="button"
                  className="button button--quiet"
                  aria-pressed={revealed.includes(index)}
                  onClick={() => {
                    setRevealed((current) =>
                      current.includes(index)
                        ? current.filter((i) => i !== index)
                        : [...current, index],
                    );
                  }}
                >
                  <span aria-hidden="true">{revealed.includes(index) ? '◉' : '◎'}</span>
                  <span className="sr-only">
                    {revealed.includes(index) ? 'Hide' : 'Show'} value of variable {index + 1}
                  </span>
                </button>
                <button
                  type="button"
                  className="button button--quiet button--danger"
                  onClick={() => {
                    setRevealed((current) => current.filter((i) => i !== index));
                    onChange({
                      ...draft,
                      variables: draft.variables.filter((_, i) => i !== index),
                    });
                  }}
                >
                  <span aria-hidden="true">×</span>
                  <span className="sr-only">Remove variable {index + 1}</span>
                </button>
              </div>
            ))}
          </div>
        )}
        <button
          type="button"
          className="button"
          onClick={() => {
            onChange({ ...draft, variables: [...draft.variables, { key: '', value: '' }] });
          }}
        >
          Add variable
        </button>
      </div>

      <div className="field">
        <label className="field__label" htmlFor={packagesId}>
          Software
        </label>
        <input
          id={packagesId}
          className="field__input"
          aria-describedby={`${packagesId}-hint`}
          value={chosen
            .map((p) => (p.version === undefined ? p.name : `${p.name}@${p.version}`))
            .join(', ')}
          onChange={(event) => {
            // The mandated entries the field does not show are carried through
            // rather than dropped and re-added by the server, so a round trip
            // through the editor is not a rewrite of what the profile holds.
            onChange({ ...draft, packages: [...implied, ...parsePackages(event.target.value)] });
          }}
        />
        <p className="field__hint" id={`${packagesId}-hint`}>
          Comma separated, optionally pinned as <code>name@version</code>.
          {mandatory.packages.length === 0 ? null : (
            <>
              {' '}
              Every session also carries{' '}
              {mandatory.packages.map((name) => (
                <Chip inline key={name}>
                  {name}
                </Chip>
              ))}
            </>
          )}
        </p>
      </div>

      <div className="field">
        <span className="field__label">Skills</span>
        <SkillPicker
          skills={skills}
          selected={draft.skills}
          mandatory={mandatory.skills}
          onChange={(selected) => {
            onChange({ ...draft, skills: [...selected] });
          }}
          error={skillsError}
        />
      </div>
    </>
  );
}

/**
 * Read the software field back into a list.
 *
 * Splitting on the last `@` rather than the first: a package name may not
 * contain one, but a scoped name begins with one, and splitting at the front
 * would turn `@scope/pkg` into a pin of nothing.
 */
function parsePackages(raw: string): ProfileDraft['packages'] {
  return raw
    .split(',')
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0)
    .map((entry) => {
      const at = entry.lastIndexOf('@');
      if (at <= 0) {
        return { name: entry };
      }
      return { name: entry.slice(0, at), version: entry.slice(at + 1) };
    });
}
