import { Button } from '../ui/Button';
import type { usePreparationDraft } from './usePreparationDraft';

export function PreparationDraft({
  draft,
}: {
  readonly draft: ReturnType<typeof usePreparationDraft>;
}) {
  return (
    <div className="preparation-draft">
      <label>
        Follow-up draft
        <textarea
          value={draft.text}
          onChange={(event) => draft.change(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) event.preventDefault();
          }}
          placeholder="Keep a follow-up here while setup finishes"
        />
      </label>
      <p>Sending opens when the cloud computer is ready.</p>
      <Button disabled>Send</Button>
      {draft.error ? (
        <div role="alert">
          <p>{draft.error}</p>
          <Button onClick={draft.retryStorage}>Retry saving draft</Button>
        </div>
      ) : null}
    </div>
  );
}
