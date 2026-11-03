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
  readonly onSend: (text: string, accepted: () => void) => void | Promise<void>;
}) {
  const active = useRef(true);
  const locked = useRef(false);
  const [state, setState] = useState<'ready' | 'sending' | 'unconfirmed' | 'accepted'>('ready');
  const [requestKey, setRequestKey] = useState<string>();
  useLayoutEffect(() => {
    active.current = true;
    return () => {
      active.current = false;
    };
  }, []);
  const retained =
    controller.state.status === 'ready' ? controller.state.v2?.journal.read() : undefined;
  if (
    state === 'accepted' ||
    (requestKey !== undefined && retained?.key === requestKey && retained.state === 'accepted')
  )
    return null;
  return (
    <section className="suggested-replies" aria-label="Next you can ask">
      <span>Next you can ask</span>
      <span className="suggested-replies__source">Scenario templates</span>
      {suggestions.items.map((item) => (
        <button
          type="button"
          key={item.id}
          disabled={disabled || state !== 'ready'}
          onClick={() => {
            if (locked.current || disabled) return;
            locked.current = true;
            setState('sending');
            void Promise.resolve(
              onSend(item.text, () => {
                if (active.current) setState('accepted');
              }),
            ).then(() => {
              if (!active.current) return;
              const input =
                controller.state.status === 'ready'
                  ? controller.state.v2?.journal.read()
                  : undefined;
              setRequestKey(input?.key);
              setState((current) => (current === 'accepted' ? current : 'unconfirmed'));
            });
          }}
        >
          {item.text}
        </button>
      ))}
    </section>
  );
}
