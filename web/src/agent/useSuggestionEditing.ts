import { useLayoutEffect, useRef, useState } from 'react';
import type { ComposerSuggestionEditorHandle } from './ComposerSuggestionEditor';
import type { AgentController } from './controllerTypes';
import type { messageSubmission } from './messageSubmission';
import type { SuggestedReplySet } from './suggestions';
import { confirmSuggestionSend, rememberSuggestionRequest } from './suggestionMetrics';

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
  const [edited, setEdited] = useState<{ set: SuggestedReplySet; itemId: string }>();
  const [dismissed, setDismissed] = useState<SuggestedReplySet>();
  const retained = controller.readSuggestionRequest?.();
  const consumed =
    suggestions !== undefined &&
    retained?.state === 'accepted' &&
    retained.answerContext?.turn_sequence === suggestions.scope.turn_sequence &&
    retained.answerContext?.answer_sequence === suggestions.scope.answer_sequence;
  const source = consumed || sameSet(suggestions, dismissed) ? undefined : suggestions;
  const currentSource = useRef(source);
  useLayoutEffect(() => {
    currentSource.current = source;
    return () => {
      currentSource.current = undefined;
    };
  }, [source]);
  if (consumed && !sameSet(suggestions, dismissed)) setDismissed(suggestions);
  if (edited && !sameSet(edited.set, source)) setEdited(undefined);
  return {
    source,
    editorRef,
    edit(text: string) {
      const item = source?.items.find((candidate) => candidate.text === text);
      if (source && item)
        editorRef.current?.edit(text, (applied) =>
          setEdited(applied ? { set: source, itemId: item.id } : undefined),
        );
    },
    send(submit: ReturnType<typeof messageSubmission>, content: string, accepted: () => void) {
      const selection = sameSet(source, edited?.set) ? edited : undefined;
      const scope = selection?.set.scope;
      const request = submit(
        content,
        (receipt) => {
          if (selection) confirmSuggestionSend(selection.set, selection.itemId, 'edited', receipt);
          accepted();
        },
        scope,
        (reason) => {
          if (reason && scope) setDismissed(source);
        },
      );
      return Promise.resolve(request).then(() => {
        if (!selection || !sameSet(selection.set, currentSource.current)) return;
        const input = controller.readSuggestionRequest?.();
        if (
          input?.input !== content ||
          input.answerContext?.turn_sequence !== scope?.turn_sequence ||
          input.answerContext?.answer_sequence !== scope?.answer_sequence
        )
          return;
        if (input.state === 'pending')
          rememberSuggestionRequest(selection.set, selection.itemId, 'edited', input.key);
        else
          confirmSuggestionSend(
            selection.set,
            selection.itemId,
            'edited',
            input.receipt ?? undefined,
          );
      });
    },
  };
}
