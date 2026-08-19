import { Button } from '../ui/Button';
import { useId, useState } from 'react';
import { ServicePicker } from './ServicePicker';
import { SkillPicker } from './SkillPicker';
import { Chip } from '../ui/Chip';
import { Select } from '../ui/Select';
import { CREDENTIALS, EFFORTS, PERMISSIONS } from '../sessions/options';
import { kernelLabel, modesOf, planeRefusal, PLANES } from '../sessions/kernels';
import type { KernelOffer, SessionPlane } from '../sessions/types';
import type {
  CatalogSkill,
  MandatoryEntries,
  PlatformService,
  ProfileDraft,
  ProfileVariable,
} from './types';
import { isValidKey } from './types';

/**
 * The profile form, in two halves.
 *
 * **Agent configuration** is what the agent is and how it behaves: which
 * agent, which model, how hard it thinks, how often it stops to ask, whose
 * model access it spends, and what it knows how to do. **Environment
 * configuration** is the box around it: software, and variables.
 *
 * Split because they answer different questions, and because a profile is now
 * the whole description of a session — these settings were once chosen again
 * on every start, which left two sessions comparable only by reading both
 * forms.
 *
 * Controlled by its parent so the dialog's footer owns the save action — a
 * form whose submit button lives outside it is the price of the dialog pattern
 * in docs/Web_Style.md, and the alternative is two competing action rows.
 */

interface ProfileEditorProps {
  readonly draft: ProfileDraft;
  readonly onChange: (draft: ProfileDraft) => void;
  /**
   * The runtimes this deployment can run, and what each one can be.
   *
   * Passed in rather than read here: it describes the deployment, and the one
   * answer serves every form that needs it. Empty means the list could not be
   * read, which refuses nothing — see `modesOf`.
   */
  readonly kernels: readonly KernelOffer[];
  readonly skills: readonly CatalogSkill[];
  readonly skillsError?: string | undefined;
  readonly services: readonly PlatformService[];
  readonly mandatoryServices: readonly string[];
  readonly servicesError?: string | undefined;
  readonly mandatory: MandatoryEntries;
  /** What went wrong with the last attempt to save, if anything. */
  readonly error?: string | undefined;
}

export function ProfileEditor({
  draft,
  onChange,
  kernels,
  skills,
  skillsError,
  services,
  mandatoryServices,
  servicesError,
  mandatory,
  error,
}: ProfileEditorProps) {
  const nameId = useId();
  const packagesId = useId();
  const kernelId = useId();
  const planeId = useId();
  const modelId = useId();
  const effortId = useId();
  const permissionId = useId();
  const credentialId = useId();
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
  // Read while the form is open rather than after a save: a pairing this
  // deployment will refuse is worth saying beside the control that caused it.
  const refusal = planeRefusal(kernels, draft.agent.kernel, draft.agent.mode);

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

      <h3 className="editor__section">Agent configuration</h3>

      <div className="field">
        <label className="field__label" htmlFor={kernelId}>
          Agent
        </label>
        <Select
          id={kernelId}
          label="Agent"
          value={draft.agent.kernel}
          options={kernelOptions(kernels, draft.agent.kernel)}
          onChange={(kernel) => {
            onChange({ ...draft, agent: { ...draft.agent, kernel } });
          }}
        />
      </div>

      <div className="field">
        <label className="field__label" htmlFor={planeId}>
          Plane
        </label>
        {/*
         * On the profile because that is where the core keeps it: a session
         * cannot be moved onto the other plane without being rotated, so two
         * sessions started from one profile must agree about which they are.
         */}
        <Select
          id={planeId}
          label="Plane"
          value={draft.agent.mode}
          options={planeOptions(kernels, draft.agent.kernel)}
          onChange={(mode: SessionPlane) => {
            onChange({ ...draft, agent: { ...draft.agent, mode } });
          }}
        />
        <span className="field__hint">
          A terminal is a screen someone types at; a conversation is messages, tool calls and
          approvals. Fixed for a session's life.
        </span>
        {refusal === undefined ? null : (
          <p className="field__error" role="alert">
            {refusal}
          </p>
        )}
      </div>

      <div className="field">
        <label className="field__label" htmlFor={modelId}>
          Model
        </label>
        <input
          id={modelId}
          className="field__input"
          aria-describedby={`${modelId}-hint`}
          value={draft.agent.model ?? ''}
          onChange={(event) => {
            const next = event.target.value.trim();
            onChange({
              ...draft,
              agent: { ...draft.agent, model: next.length === 0 ? undefined : next },
            });
          }}
        />
        <p className="field__hint" id={`${modelId}-hint`}>
          Leave empty to use whatever this deployment answers with.
        </p>
      </div>

      <div className="field">
        <label className="field__label" htmlFor={effortId}>
          Thinking effort
        </label>
        <Select
          id={effortId}
          label="Thinking effort"
          value={draft.agent.reasoningEffort}
          options={EFFORTS.map((e) => ({
            value: e.id,
            label: e.hint === '' ? e.label : `${e.label} — ${e.hint}`,
          }))}
          onChange={(reasoningEffort) => {
            onChange({ ...draft, agent: { ...draft.agent, reasoningEffort } });
          }}
        />
      </div>

      <div className="field">
        <label className="field__label" htmlFor={permissionId}>
          Permissions
        </label>
        <Select
          id={permissionId}
          label="Permissions"
          value={draft.agent.permission}
          options={PERMISSIONS.map((p) => ({ value: p.id, label: p.label }))}
          onChange={(permission) => {
            onChange({ ...draft, agent: { ...draft.agent, permission } });
          }}
        />
      </div>

      <div className="field">
        <label className="field__label" htmlFor={credentialId}>
          Model access
        </label>
        <Select
          id={credentialId}
          label="Model access"
          value={draft.agent.credential}
          options={CREDENTIALS.map((c) => ({ value: c.id, label: `${c.label} — ${c.hint}` }))}
          onChange={(credential) => {
            onChange({ ...draft, agent: { ...draft.agent, credential } });
          }}
        />
      </div>

      <div className="field">
        <span className="field__label">Skills</span>
        <SkillPicker
          skills={skills}
          selected={draft.agent.skills}
          mandatory={mandatory.skills}
          onChange={(selected) => {
            onChange({ ...draft, agent: { ...draft.agent, skills: [...selected] } });
          }}
          error={skillsError}
        />
      </div>

      <h3 className="editor__section">NyxID</h3>

      <div className="field">
        <span className="field__label">Services this session may reach</span>
        <p className="field__hint">
          A session is given its own credential rather than yours, and it reaches only what is
          ticked here. Everything else on your account stays out of its reach.
        </p>
        <ServicePicker
          services={services}
          selected={draft.nyxid.services}
          mandatory={mandatoryServices}
          onChange={(selected) => {
            onChange({ ...draft, nyxid: { services: [...selected] } });
          }}
          error={servicesError}
        />
      </div>

      <h3 className="editor__section">Environment configuration</h3>
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
                <Button
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
                </Button>
                <Button
                  variant="primary"
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
                </Button>
              </div>
            ))}
          </div>
        )}
        <Button
          onClick={() => {
            onChange({ ...draft, variables: [...draft.variables, { key: '', value: '' }] });
          }}
        >
          Add variable
        </Button>
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

/**
 * The runtimes on offer, plus whichever one this profile already names.
 *
 * A profile naming a runtime the deployment no longer runs keeps it in the
 * list, marked, rather than silently reading as something else. Where nothing
 * is known — the list could not be read — the profile's own runtime is offered
 * without a mark, because refusing on the strength of a failed request would
 * empty a form over a network error.
 */
function kernelOptions(kernels: readonly KernelOffer[], chosen: string) {
  const offered = kernels.map((kernel) => ({
    value: kernel.id,
    label: kernelLabel(kernel.id),
    unavailable: kernel.modes.length === 0 ? 'Runs no sessions' : undefined,
  }));
  if (offered.some((option) => option.value === chosen)) {
    return offered;
  }
  return [
    {
      value: chosen,
      label: kernelLabel(chosen),
      unavailable: kernels.length === 0 ? undefined : 'Not run by this deployment',
    },
    ...offered,
  ];
}

/** The planes this runtime serves, with the rest shown refused rather than hidden (§7). */
function planeOptions(kernels: readonly KernelOffer[], kernel: string) {
  const modes = modesOf(kernels, kernel);
  return PLANES.map((plane) => ({
    value: plane.id,
    label: plane.label,
    unavailable:
      modes === undefined || modes.includes(plane.id) ? undefined : `Not on ${kernelLabel(kernel)}`,
  }));
}
