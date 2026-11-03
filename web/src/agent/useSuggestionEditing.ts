import { useRef, useState } from 'react';
import type { ComposerSuggestionEditorHandle } from './ComposerSuggestionEditor';
import type { AgentController } from './controllerTypes';
import type { messageSubmission } from './messageSubmission';
import type { SuggestedReplySet } from './suggestions';

function sameSet(left: SuggestedReplySet | undefined, right: SuggestedReplySet | undefined) {
  return (
    left !== undefined &&
    right !== undefined &&
    left.id === right.id &&
    left.viewerId === right.viewerId &&
    left.agentId === right.agentId &&
    left.answerId === right.answerId &&
    left.scope.turn_sequence === right.scope.turn_sequence &&
    left.scope.answer_sequence === right.scope.answer_sequence &&
    left.source.kind === right.source.kind &&
    left.source.version === right.source.version
  );
}

export function useSuggestionEditing(
  suggestions: SuggestedReplySet | undefined,
  controller: AgentController,
) {
  const editorRef = useRef<ComposerSuggestionEditorHandle>(null);
  const [edited, setEdited] = useState<SuggestedReplySet>();
  const [dismissed, setDismissed] = useState<SuggestedReplySet>();
  const retained = controller.readSuggestionRequest?.();
  const consumed =
    suggestions !== undefined &&
    retained?.state === 'accepted' &&
    retained.answerContext?.turn_sequence === suggestions.scope.turn_sequence &&
    retained.answerContext?.answer_sequence === suggestions.scope.answer_sequence;
  const source = consumed || sameSet(suggestions, dismissed) ? undefined : suggestions;
  if (consumed && !sameSet(suggestions, dismissed)) setDismissed(suggestions);
  if (edited && !sameSet(edited, source)) setEdited(undefined);
  return {
    source,
    editorRef,
    edit(text: string) {
      if (source)
        editorRef.current?.edit(text, (applied) => setEdited(applied ? source : undefined));
    },
    send(submit: ReturnType<typeof messageSubmission>, content: string, accepted: () => void) {
      const scope = sameSet(source, edited) ? source?.scope : undefined;
      return submit(content, accepted, scope, (reason) => {
        if (reason && scope) setDismissed(source);
      });
    },
  };
}
