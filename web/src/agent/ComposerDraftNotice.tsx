import type { AgentDraft } from './draft';
import { Button } from '../ui/Button';

export function ComposerDraftNotice({ draft }: { readonly draft?: AgentDraft }) {
  if (!draft?.error) return null;
  return (
    <div className="agent-composer__notice" role="alert">
      <p>{draft.error}</p>
      <Button size="sm" onClick={draft.retryStorage}>
        Retry draft storage
      </Button>
      <Button size="sm" onClick={draft.discardSaved}>
        Discard saved draft
      </Button>
    </div>
  );
}
