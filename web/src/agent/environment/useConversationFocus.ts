import { useContext, useEffect, type RefObject } from 'react';
import { ComputerContext } from './context';

export function useConversationFocus(
  input: RefObject<HTMLTextAreaElement | null>,
  available: boolean,
) {
  const observation = useContext(ComputerContext);
  useEffect(() => {
    if (available && observation)
      return observation.offerConversationFocus(() => input.current?.focus());
  }, [available, input, observation]);
}
