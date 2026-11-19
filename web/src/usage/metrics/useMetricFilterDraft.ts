import { useState } from 'react';
import type { Selection } from './model';

export function useMetricFilterDraft(selection: Selection) {
  const [stored, setStored] = useState({ selection, draft: selection, error: '' });
  if (stored.selection !== selection) setStored({ selection, draft: selection, error: '' });
  const current =
    stored.selection === selection ? stored : { selection, draft: selection, error: '' };
  const { draft, error } = current;
  const changed = (Object.keys(selection) as (keyof Selection)[]).some(
    (field) => draft[field] !== selection[field],
  );
  return {
    draft,
    error,
    changed,
    update(value: Selection) {
      setStored((previous) => ({
        selection,
        draft: value,
        error:
          previous.selection === selection &&
          previous.draft.range === value.range &&
          previous.draft.from === value.from &&
          previous.draft.to === value.to
            ? previous.error
            : '',
      }));
    },
    setError(message: string) {
      setStored((previous) => ({ ...previous, error: message }));
    },
  };
}
