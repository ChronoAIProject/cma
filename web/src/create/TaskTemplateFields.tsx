import { useEffect, useState } from 'react';
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

export function useTaskTemplateSupport(client: CmaWebClient) {
  const [result, setResult] = useState<{ client: CmaWebClient; supported: boolean }>();
  useEffect(() => {
    const abort = new AbortController();
    if (!client.readTaskTemplateSupport) return;
    void client.readTaskTemplateSupport({ signal: abort.signal }).then(
      (value) => {
        if (!abort.signal.aborted) setResult({ client, supported: value });
      },
      () => {
        if (!abort.signal.aborted) setResult({ client, supported: false });
      },
    );
    return () => abort.abort();
  }, [client]);
  return client.readTaskTemplateSupport
    ? result?.client === client
      ? result.supported
      : undefined
    : false;
}
export function TaskTemplateFields({
  template,
  issue,
  errorId,
  onChange,
  onClear,
}: {
  readonly template: TaskTemplate;
  readonly issue?: TemplateIssue;
  readonly errorId: string;
  readonly onChange: (template: TaskTemplate) => void;
  readonly onClear: () => void;
}) {
  const definition = templateDefinition(template.id);
  const fields: Readonly<Record<string, string>> = template.fields;
  return (
    <fieldset className="task-template-fields">
      <legend>{definition.name}</legend>
      <p>Fill in every field to preview and start your task.</p>
      {definition.fields.map(({ key, label }) => (
        <label key={key} className="task-template-fields__field">
          <span>{label}</span>
          <Input
            data-template-field={key}
            value={fields[key] ?? ''}
            required
            aria-invalid={issue?.field === key || undefined}
            aria-describedby={issue?.field === key ? errorId : undefined}
            onChange={(event) => onChange(changeTemplateField(template, key, event.target.value))}
          />
        </label>
      ))}
      <Button variant="outline" onClick={onClear}>
        Write a task without a template
      </Button>
    </fieldset>
  );
}
