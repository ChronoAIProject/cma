import { Button } from '../ui/Button';
import type { SavedTaskDraft } from './taskDraftStorage';
import { templateDefinition } from './taskTemplateModel';

export function TaskDraftRecovery({
  drafts,
  onRestore,
}: {
  readonly drafts: readonly SavedTaskDraft[];
  readonly onRestore: (id: string) => void;
}) {
  if (!drafts.length) return null;
  return (
    <details className="task-home__status">
      <summary>Other saved task drafts ({drafts.length})</summary>
      <p>These drafts were kept separately. Using one also keeps your current draft.</p>
      {drafts.map(({ id, draft }, index) => {
        const templates = [
          ...(draft.templates ?? []).filter((template) => template.id !== draft.template?.id),
          ...(draft.template ? [draft.template] : []),
        ];
        const details = [
          ...(draft.text ? [`Task: ${draft.text}`] : []),
          ...templates.map((template) => {
            const fields: Readonly<Record<string, string>> = template.fields;
            const definition = templateDefinition(template.id);
            return `${definition.name}\n${definition.fields.map(({ key, label }) => `${label}: ${fields[key] ?? ''}`).join('\n')}`;
          }),
          ...(draft.repositoryUrl
            ? [`Repository: ${draft.repositoryUrl}`, `Ref: ${draft.repositoryRef || 'default'}`]
            : []),
          `Shared desktop: ${draft.machineIncluded ? 'included' : 'not included'}`,
        ].join('\n\n');
        return (
          <section key={id} aria-label={`Saved draft ${index + 1}`}>
            <label>
              Saved draft {index + 1} details
              <textarea className="input" readOnly value={details} />
            </label>
            <Button variant="outline" onClick={() => onRestore(id)}>
              Use this saved draft
            </Button>
          </section>
        );
      })}
    </details>
  );
}
