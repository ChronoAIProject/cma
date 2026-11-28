import { Button } from '../ui/Button';

export function PreparationDraft({
  draft,
  unavailableReason = 'Sending opens when the cloud computer is ready.',
}: {
  readonly unavailableReason?: string;
  readonly draft: {
    readonly text: string;
    readonly change: (text: string) => void;
    readonly error?: string;
    readonly retryStorage?: () => void;
  };
}) {
  return (
    <div className="preparation-draft">
      <textarea
        aria-label="Follow-up message"
        value={draft.text}
        onChange={(event) => draft.change(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) event.preventDefault();
        }}
        placeholder="Keep a follow-up here while setup finishes"
      />
      <p>{unavailableReason}</p>
      <Button disabled>Send</Button>
      {draft.error ? (
        <div role="alert">
          <p>{draft.error}</p>
          {draft.retryStorage ? (
            <Button onClick={draft.retryStorage}>Retry saving draft</Button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
