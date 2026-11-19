import {
  needsNavigationConfirmation,
  requestNavigation,
  type NavigationTarget,
} from './navigationGuard';

const marker = 'cmaNavigation';
interface Position {
  readonly session: string;
  readonly index: number;
}
interface Entry extends NavigationTarget {
  readonly position?: Position;
  readonly browserIndex?: number;
}
function position(state: unknown): Position | undefined {
  if (!state || typeof state !== 'object' || !(marker in state)) return;
  const value = state[marker];
  if (
    value &&
    typeof value === 'object' &&
    'session' in value &&
    'index' in value &&
    typeof value.session === 'string' &&
    Number.isSafeInteger(value.index)
  )
    return value as Position;
}
function fields(state: unknown): Record<string, unknown> {
  return state && typeof state === 'object' ? { ...state } : {};
}
function entry(): Entry {
  const navigation = (
    window as Window & {
      navigation?: { currentEntry?: { index: number } };
    }
  ).navigation;
  const index = navigation?.currentEntry?.index;
  return {
    url: window.location.pathname + window.location.search + window.location.hash,
    state: window.history.state,
    position: position(window.history.state),
    browserIndex: index !== undefined && index >= 0 ? index : undefined,
  };
}
function distance(from: Entry, to: Entry) {
  if (from.browserIndex !== undefined && to.browserIndex !== undefined)
    return to.browserIndex - from.browserIndex;
  if (from.position && to.position?.session === from.position.session)
    return to.position.index - from.position.index;
}

export function browserHistory(onChange: () => void) {
  const tag = (next: Position) => {
    window.history.replaceState({ ...fields(window.history.state), [marker]: next }, '');
  };
  if (!position(window.history.state)) tag({ session: crypto.randomUUID(), index: 0 });
  let current = entry();
  let restoring: { source: Entry; target: Entry } | undefined;
  let replay: Entry | undefined;
  let traversal: ReturnType<typeof setTimeout> | undefined;
  let live = true;
  const publish = () => {
    if (!position(window.history.state)) tag({ session: crypto.randomUUID(), index: 0 });
    current = entry();
    onChange();
    queueMicrotask(() => window.dispatchEvent(new Event('cma:navigate')));
  };
  const navigate = (url: string, state: unknown, replace = false) => {
    if (restoring || replay) return;
    current = entry();
    const target = { url, state };
    requestNavigation(target, () => {
      if (!live) return;
      const previous = current.position ?? { session: crypto.randomUUID(), index: 0 };
      const next = { ...previous, index: previous.index + (replace ? 0 : 1) };
      const nextState: Record<string, unknown> = {
        ...(replace ? fields(current.state) : {}),
        ...fields(state),
        [marker]: next,
      };
      if (!('usageBackground' in fields(state))) delete nextState.usageBackground;
      window.history[replace ? 'replaceState' : 'pushState'](nextState, '', url);
      publish();
    });
  };
  const scheduleReplay = () => {
    clearTimeout(traversal);
    traversal = setTimeout(() => {
      if (!live || !replay) return;
      const remaining = distance(entry(), replay);
      if (remaining) window.history.go(remaining);
      else {
        replay = undefined;
        restoring = undefined;
        publish();
      }
    }, 0);
  };
  const confirmTraversal = (source: Entry, target: Entry) => {
    requestNavigation(target, () => {
      if (!live) return;
      const delta = distance(source, target);
      if (delta !== undefined && delta !== 0) {
        replay = target;
        scheduleReplay();
      } else {
        // Legacy entries without either coordinate cannot be safely traversed by guessing.
        window.history.replaceState(target.state, '', target.url);
        publish();
      }
    });
  };
  const onPop = () => {
    const target = entry();
    if (replay) {
      const delta = distance(target, replay);
      restoring = undefined;
      if (delta !== undefined && delta !== 0) {
        scheduleReplay();
        return;
      }
      clearTimeout(traversal);
      replay = undefined;
      publish();
      return;
    }
    if (restoring) {
      const { source, target: destination } = restoring;
      const delta = distance(target, source);
      if (delta !== undefined && delta !== 0) {
        window.history.go(delta);
        return;
      }
      restoring = undefined;
      current = target;
      confirmTraversal(source, destination);
      return;
    }
    if (!needsNavigationConfirmation(target)) {
      publish();
      return;
    }
    const source = current;
    const delta = distance(target, source);
    if (delta !== undefined && delta !== 0) {
      restoring = { source, target };
      window.history.go(delta);
    } else {
      window.history.replaceState(source.state, '', source.url);
      confirmTraversal(source, target);
    }
  };
  const onRequest = (event: Event) => {
    event.preventDefault();
    navigate((event as CustomEvent<string>).detail, null);
  };
  const onProtection = () => {
    if (!restoring && !replay) current = entry();
  };
  window.addEventListener('popstate', onPop);
  window.addEventListener('cma:route-request', onRequest);
  window.addEventListener('cma:protect-navigation', onProtection);
  return {
    navigate,
    dispose: () => {
      live = false;
      clearTimeout(traversal);
      window.removeEventListener('popstate', onPop);
      window.removeEventListener('cma:route-request', onRequest);
      window.removeEventListener('cma:protect-navigation', onProtection);
    },
  };
}
