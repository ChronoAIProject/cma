import { useLayoutEffect, useRef, useState } from 'react';
import type { SuggestedReplySet } from './suggestions';
import type { AgentController } from './controller';
import './SuggestedReplies.css';

export function SuggestedReplies({
  suggestions,
  controller,
  disabled,
  onSend,
}: {
  readonly suggestions: SuggestedReplySet;
  readonly controller: AgentController;
  readonly disabled: boolean;
  readonly onSend: (
    text: string,
    accepted: () => void,
    refused: (reason?: 'answer_changed') => void,
  ) => void | Promise<void>;
}) {
  const active = useRef(true);
  const locked = useRef(false);
  const [state, setState] = useState<
    'ready' | 'sending' | 'unconfirmed' | 'accepted' | 'failed' | 'expired'
  >('ready');
  const [requestKey, setRequestKey] = useState<string>();
  useLayoutEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
    };
  }, []);
  const retained = controller.readSuggestionRequest?.();
  const scopedRequest =
    retained?.answerContext?.turn_sequence === suggestions.scope.turn_sequence &&
    retained?.answerContext?.answer_sequence === suggestions.scope.answer_sequence;
  const effectiveState =
    state === 'unconfirmed' && requestKey !== undefined && !retained ? 'failed' : state;
  const status =
    effectiveState === 'sending'
      ? 'Sending suggestion…'
      : effectiveState === 'unconfirmed' || (scopedRequest && retained.state === 'pending')
        ? 'Awaiting confirmation — recover the pending request.'
        : effectiveState === 'failed'
          ? 'Suggestion not sent. Try again.'
          : undefined;
  if (
    state === 'accepted' ||
    state === 'expired' ||
    (requestKey !== undefined && retained?.key === requestKey && retained.state === 'accepted')
  )
    return null;
  return (
    <section className="suggested-replies" aria-label="Next you can ask">
      <span>Next you can ask</span>
      <span className="suggested-replies__source">
        {suggestions.source.kind === 'template' ? 'Scenario templates' : 'Suggested follow-ups'}
      </span>
      {status && (
        <span role="status" aria-live="polite">
          {status}
        </span>
      )}
      {suggestions.items.map((item) => (
        <button
          type="button"
          key={item.id}
          disabled={
            disabled ||
            (scopedRequest && retained.state === 'pending') ||
            (effectiveState !== 'ready' && effectiveState !== 'failed')
          }
          onClick={() => {
            if (
              (locked.current && effectiveState !== 'failed') ||
              disabled ||
              (scopedRequest && retained.state === 'pending')
            )
              return;
            locked.current = true;
            setState('sending');
            void Promise.resolve(
              onSend(
                item.text,
                () => {
                  if (active.current) setState('accepted');
                },
                (reason) => {
                  if (!active.current) return;
                  locked.current = false;
                  setState(reason === 'answer_changed' ? 'expired' : 'failed');
                },
              ),
            ).then(() => {
              if (!active.current) return;
              const input = controller.readSuggestionRequest?.();
              setRequestKey(input?.key);
              setState((current) =>
                current === 'accepted' || current === 'expired' || current === 'failed'
                  ? current
                  : 'unconfirmed',
              );
            });
          }}
        >
          {item.text}
        </button>
      ))}
    </section>
  );
}
