import { Input } from '../ui/Input';
import type { SandboxProfileAvailability, SandboxProfileContent } from './client';
import { Field } from './Field';
import { fieldProps, type EditorState } from './ProfileEditor';

export function SandboxAvailabilityFields({
  editor,
}: {
  readonly editor: EditorState<SandboxProfileContent>;
}) {
  const value = editor.draft.availability ?? {
    warmupEnabled: false,
    warmupCount: 1,
    alwaysKeepAlive: false,
  };
  const publicProfile =
    editor.profile?.visibility === 'public' && editor.profile.latestRevision !== undefined;
  const change = (next: Partial<SandboxProfileAvailability>) =>
    editor.change({ ...editor.draft, availability: { ...value, ...next } });
  return (
    <section className="profile-editor__band">
      <h2>Availability</h2>
      <ul className="profile-service-list">
        <li>
          <label>
            <input
              {...fieldProps(editor, 'warmupEnabled')}
              type="checkbox"
              checked={value.warmupEnabled}
              disabled={editor.locked || !publicProfile}
              onChange={(event) => change({ warmupEnabled: event.target.checked })}
            />
            <span>
              Enable warm up
              <small id="profile-warmupEnabled-help">
                {publicProfile
                  ? 'Prepare sandboxes in advance for faster starts. Your accounts are connected only when you start your sandbox.'
                  : 'Save, publish and make this profile public before enabling warm up.'}
              </small>
            </span>
          </label>
        </li>
        <li>
          <label>
            <input
              {...fieldProps(editor, 'alwaysKeepAlive')}
              type="checkbox"
              checked={value.alwaysKeepAlive}
              onChange={(event) => change({ alwaysKeepAlive: event.target.checked })}
            />
            <span>
              Always keep alive
              <small id="profile-alwaysKeepAlive-help">
                Keep existing and new sandboxes from this profile live when idle. You can still
                pause them manually.
              </small>
            </span>
          </label>
        </li>
      </ul>
      <Field
        id="profile-warmupCount"
        label="Warm sandboxes"
        help="Keep between 1 and 5 sandboxes prepared. CMA prepares a replacement whenever someone starts one."
        error={editor.errors.warmupCount}
      >
        <Input
          {...fieldProps(editor, 'warmupCount')}
          type="number"
          min={1}
          max={5}
          step={1}
          value={Number.isFinite(value.warmupCount) ? value.warmupCount : ''}
          disabled={editor.locked || !publicProfile || !value.warmupEnabled}
          onChange={(event) => change({ warmupCount: event.target.valueAsNumber })}
        />
      </Field>
      {editor.errors.warmupEnabled === undefined ? null : (
        <p id="profile-warmupEnabled-error" className="profile-field__error">
          {editor.errors.warmupEnabled}
        </p>
      )}
      <p className="profile-field__help">
        Availability changes apply when you save. Your sandboxes keep their existing environment and
        work.
      </p>
    </section>
  );
}
