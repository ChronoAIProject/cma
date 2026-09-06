import { Button } from '../../ui/Button';
import { Input } from '../../ui/Input';
import { IconPlusOutline16, IconTrashOutline16 } from '../../ui/icons';
import type { SandboxProfileContent } from '../client';
import { Field } from '../Field';
import { fieldProps, type EditorState } from '../ProfileEditor';

export function EnvironmentEditor({
  editor,
}: {
  readonly editor: EditorState<SandboxProfileContent>;
}) {
  const value = editor.draft.content.environmentVariables;
  const change = (next: typeof value) =>
    editor.change({
      ...editor.draft,
      content: { ...editor.draft.content, environmentVariables: next },
    });
  return (
    <div className="profile-field">
      <p className="profile-field__help">
        Use these values for configuration, never secrets; public profiles expose every value.
      </p>
      {value.map((entry, index) => {
        const prefix = `environmentVariables.${String(index)}`;
        return (
          <div key={index} className="profile-environment-row">
            <Field
              id={`profile-${prefix}.name`}
              label={`Variable ${String(index + 1)} name`}
              help="Use a non-reserved uppercase name, with numbers or underscores after the first character."
              error={editor.errors[`${prefix}.name`]}
            >
              <Input
                {...fieldProps(editor, `${prefix}.name`)}
                value={entry.name}
                onChange={(event) =>
                  change(
                    value.map((current, at) =>
                      at === index ? { ...current, name: event.target.value } : current,
                    ),
                  )
                }
              />
            </Field>
            <Field
              id={`profile-${prefix}.value`}
              label={`Variable ${String(index + 1)} value`}
              help="Enter a plain configuration value that is safe to share."
              error={editor.errors[`${prefix}.value`]}
            >
              <Input
                {...fieldProps(editor, `${prefix}.value`)}
                value={entry.value}
                onChange={(event) =>
                  change(
                    value.map((current, at) =>
                      at === index ? { ...current, value: event.target.value } : current,
                    ),
                  )
                }
              />
            </Field>
            <Button
              type="button"
              size="sm"
              icon={<IconTrashOutline16 />}
              title={`Remove variable ${String(index + 1)}`}
              aria-label={`Remove variable ${String(index + 1)}`}
              disabled={editor.locked}
              onClick={() => change(value.filter((_, at) => at !== index))}
            />
          </div>
        );
      })}
      {editor.errors.environmentVariables === undefined ? null : (
        <p className="profile-field__error">{editor.errors.environmentVariables}</p>
      )}
      <Button
        type="button"
        icon={<IconPlusOutline16 />}
        disabled={editor.locked || value.length >= 64}
        onClick={() => change([...value, { name: '', value: '' }])}
      >
        Add variable
      </Button>
    </div>
  );
}
