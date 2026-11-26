import { useEffect, useId, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import {
  changeTemplateField,
  templateDefinition,
  type TaskTemplate,
  type TemplateIssue,
} from './taskTemplateModel';
import './TaskTemplateFields.css';
import { spreadsheetLinkIssue } from './spreadsheetLinkValidation';

export type TaskTemplateAvailability = 'checking' | 'supported' | 'unsupported' | 'error';

export function useTaskTemplateAvailability(
  client: CmaWebClient,
  version: 1 | 2 = 1,
): TaskTemplateAvailability {
  const read = version === 2 ? client.readTaskTemplateV2Support : client.readTaskTemplateSupport;
  const [result, setResult] = useState<{
    client: CmaWebClient;
    version: 1 | 2;
    availability: TaskTemplateAvailability;
  }>();
  useEffect(() => {
    const abort = new AbortController();
    if (!read) return;
    void read({ signal: abort.signal }).then(
      (value) => {
        if (!abort.signal.aborted)
          setResult({
            client,
            version,
            availability: value === true ? 'supported' : 'unsupported',
          });
      },
      () => {
        if (!abort.signal.aborted) setResult({ client, version, availability: 'error' });
      },
    );
    return () => abort.abort();
  }, [client, read, version]);
  return read
    ? result?.client === client && result.version === version
      ? result.availability
      : 'checking'
    : 'unsupported';
}

export function useTaskTemplateSupport(client: CmaWebClient) {
  const availability = useTaskTemplateAvailability(client);
  return availability === 'checking' ? undefined : availability === 'supported';
}
export function TaskTemplateFields({
  template,
  issue,
  errorId,
  onChange,
  onClear,
  allowV2 = false,
}: {
  readonly template: TaskTemplate;
  readonly issue?: TemplateIssue;
  readonly errorId: string;
  readonly onChange: (template: TaskTemplate) => void;
  readonly onClear?: () => void;
  readonly allowV2?: boolean;
}) {
  const definition = templateDefinition(template.id);
  const fields: Readonly<Record<string, string>> = template.fields;
  const localErrorId = useId();
  const linkIssue = spreadsheetLinkIssue(template);
  const localIssue = issue?.field === linkIssue?.field ? undefined : linkIssue;
  return (
    <fieldset className="task-template-fields" data-template={template.id}>
      <legend>{definition.name}</legend>
      <p>Fill in every field to preview and start your task.</p>
      {definition.fields.map(({ key, label }) => (
        <label key={key} className="task-template-fields__field">
          <span>{label}</span>
          <Input
            data-template-field={key}
            value={fields[key] ?? ''}
            required
            aria-invalid={issue?.field === key || localIssue?.field === key || undefined}
            aria-describedby={
              issue?.field === key ? errorId : localIssue?.field === key ? localErrorId : undefined
            }
            onChange={(event) =>
              onChange(changeTemplateField(template, key, event.target.value, allowV2))
            }
          />
        </label>
      ))}
      {localIssue ? (
        <div
          className="task-template-fields__field task-home__alert"
          id={localErrorId}
          role="alert"
        >
          {localIssue.message}
        </div>
      ) : null}
      {onClear ? (
        <Button variant="outline" onClick={onClear}>
          Write a task without a template
        </Button>
      ) : null}
    </fieldset>
  );
}
