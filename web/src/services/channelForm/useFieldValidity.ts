import { useLayoutEffect, useRef } from 'react';

export function useFieldValidity(message?: string) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const field = ref.current?.querySelector<HTMLInputElement | HTMLTextAreaElement>(
      'input, textarea',
    );
    field?.setCustomValidity(message ?? '');
  }, [message]);
  return ref;
}
