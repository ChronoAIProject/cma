import { useLayoutEffect, useRef } from 'react';
import { animateArrival } from '../ui/arrivalMotion';
import type { ConversationState } from './model';
import type { OptimisticMessage } from './optimisticMessages';

/** History is a baseline, and tokens update an existing row rather than arriving again. */
export function useMessageArrival(
  conversation: ConversationState,
  optimistic: readonly OptimisticMessage[] | undefined,
  enabled: boolean,
) {
  const ref = useRef<HTMLDivElement>(null);
  const seen = useRef(new Set<string>());
  const initialized = useRef(false);
  const animations = useRef(new Set<Animation>());

  useLayoutEffect(() => {
    const arriving = new Set<string>();
    for (const message of conversation.messages) {
      const key = `message:${message.id}`;
      if (
        initialized.current &&
        enabled &&
        !seen.current.has(key) &&
        message.role === 'assistant' &&
        message.status === 'streaming' &&
        message.sequence === undefined &&
        message.runId !== undefined &&
        message.runId === conversation.activeRunId
      )
        arriving.add(key);
      seen.current.add(key);
    }
    for (const message of optimistic ?? []) {
      const key = `local:${message.id}`;
      if (initialized.current && enabled && !seen.current.has(key) && message.state === 'sending')
        arriving.add(key);
      seen.current.add(key);
    }
    initialized.current = true;
    if (arriving.size === 0) return;
    for (const element of ref.current?.querySelectorAll<HTMLElement>('[data-message-key]') ?? []) {
      if (!arriving.has(element.dataset.messageKey ?? '')) continue;
      const animation = animateArrival(element);
      if (animation === undefined) continue;
      const active = animations.current;
      active.add(animation);
      const release = () => {
        active.delete(animation);
        animation.removeEventListener('finish', release);
        animation.removeEventListener('cancel', release);
      };
      animation.addEventListener('finish', release);
      animation.addEventListener('cancel', release);
    }
  }, [conversation, optimistic, enabled]);

  useLayoutEffect(() => {
    const active = animations.current;
    return () => {
      for (const animation of active) animation.cancel();
      active.clear();
    };
  }, []);
  return ref;
}
