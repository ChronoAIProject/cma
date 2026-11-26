import { useState } from 'react';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';
import { compileTaskTemplate, templateDefinition, type TaskTemplateId } from './taskTemplateModel';
import type { useTaskDraft } from './taskDraft';
import { TaskDraftRecovery } from './TaskDraftRecovery';

export function useTaskTemplateDraftSwitch(
  draft: ReturnType<typeof useTaskDraft>,
  focus: (template: boolean) => void,
  clearValidation: () => void,
) {
  const [confirmation, setConfirmation] = useState<'partial' | 'replace'>();
  const [copy, setCopy] = useState<'copied' | 'failed'>();
  const select = (id: TaskTemplateId) => {
    if (!draft.selectTemplate(id)) return;
    clearValidation();
    focus(true);
  };
  const switchToText = () => {
    if (!draft.clearTemplate()) return;
    setConfirmation(undefined);
    clearValidation();
    focus(false);
  };
  const clear = () => {
    setCopy(undefined);
    const compiled = draft.template && compileTaskTemplate(draft.template);
    const hasFields = draft.template && Object.values(draft.template.fields).some(Boolean);
    if (compiled?.issue && hasFields) setConfirmation('partial');
    else if (!compiled?.issue && draft.text && draft.text !== compiled?.text)
      setConfirmation('replace');
    else switchToText();
  };
  const filled = draft.template
    ? templateDefinition(draft.template.id)
        .fields.map(({ key, label }) => {
          const value = (draft.template!.fields as Readonly<Record<string, string>>)[key];
          return value ? `${label}: ${value}` : '';
        })
        .filter(Boolean)
        .join('\n')
    : draft.text;
  const copyFields = (
    <>
      <Button
        onClick={() => {
          void navigator.clipboard?.writeText(filled).then(
            () => setCopy('copied'),
            () => setCopy('failed'),
          );
          if (!navigator.clipboard) setCopy('failed');
        }}
      >
        Copy filled details
      </Button>
      {copy === 'copied' ? <p role="status">Filled details copied.</p> : null}
      {copy === 'failed' ? (
        <label>
          Copy is unavailable. Select and copy your filled details.
          <textarea className="input" readOnly value={filled} />
        </label>
      ) : null}
    </>
  );
  return {
    select,
    clear,
    feedback: (
      <>
        <TaskDraftRecovery
          drafts={draft.recoverable}
          onRestore={(id) => {
            const saved = draft.recoverable.find((value) => value.id === id);
            if (!saved || !draft.restoreSavedDraft(id)) return;
            clearValidation();
            focus(Boolean(saved.draft.template));
          }}
        />
        {!draft.template &&
          draft.templates?.map((template) => (
            <div key={template.id} className="task-home__status">
              <p>Your {template.id} template draft is kept separately.</p>
              <Button variant="outline" onClick={() => select(template.id)}>
                Resume {template.id} template
              </Button>
            </div>
          ))}
        {draft.error && !confirmation ? copyFields : null}
        <Modal
          open={confirmation !== undefined}
          title={
            confirmation === 'replace'
              ? 'Replace your free-text draft?'
              : 'Keep this template draft?'
          }
          description={
            confirmation === 'replace'
              ? 'Converting will replace your separate free-text draft with the complete task preview. Your template fields will be kept.'
              : 'Your template is not complete yet. Switching will open your separate free-text draft.'
          }
          onClose={() => setConfirmation(undefined)}
          footer={
            <>
              <Button variant="primary" onClick={() => setConfirmation(undefined)}>
                Keep editing template
              </Button>
              <Button variant="outline" onClick={switchToText}>
                {confirmation === 'replace' ? 'Replace free-text draft' : 'Switch and keep draft'}
              </Button>
            </>
          }
        >
          <p>Template fields, repository and computer choices stay with this task draft.</p>
          {draft.error ? (
            <>
              <p role="alert">{draft.error}</p>
              {copyFields}
            </>
          ) : null}
        </Modal>
      </>
    ),
  };
}
