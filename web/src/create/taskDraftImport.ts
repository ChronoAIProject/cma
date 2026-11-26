import { hasPendingLifecycleIntent } from './lifecycleIntentStorage';
import { taskDraftIsMounted } from './taskDraft';
import { openTaskDraftStore } from './taskDraftStorage';

export interface HomeRequestImport {
  readonly conflict: boolean;
  readonly commit: (replace: boolean) => boolean;
}

/** A retained request supplies text only; the Home admission owner creates any future command. */
export function prepareHomeRequestImport(
  owner: string,
  text: string,
  authorized: () => boolean,
): HomeRequestImport {
  const store = openTaskDraftStore(owner);
  const assertAvailable = () => {
    if (!authorized()) throw new Error('Access to this request changed. Open the Agent again.');
    if (taskDraftIsMounted(store.key))
      throw new Error(
        'Your current task draft is open. Return to it before importing this request.',
      );
    if (hasPendingLifecycleIntent({ owner }))
      throw new Error(
        'Resolve the original pending task request before importing another request.',
      );
  };
  assertAvailable();
  if (!text || new TextEncoder().encode(text).length > 256 * 1024)
    throw new Error('Original request unavailable');
  const saved = store.read();
  if (saved.error) throw new Error(saved.error);
  const draft = saved.draft;
  const conflict = Boolean(
    draft.text ||
    draft.template ||
    draft.templates?.length ||
    draft.repositoryUrl ||
    draft.repositoryRef ||
    draft.machineIncluded,
  );
  let attempted = false;
  return {
    conflict,
    commit(replace) {
      if (attempted || (conflict && !replace)) return false;
      assertAvailable();
      attempted = true;
      store.importText(text, conflict);
      assertAvailable();
      return true;
    },
  };
}
