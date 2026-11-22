export interface NavigationTarget {
  readonly url: string;
  readonly state: unknown;
}

interface NavigationGuard {
  readonly leaves: (target: NavigationTarget) => boolean;
  readonly confirm: (proceed: () => void) => void;
  readonly release: () => void;
}

let active: NavigationGuard | undefined;

export function releaseNavigationGuard() {
  const guard = active;
  active = undefined;
  guard?.release();
}

export function registerNavigationGuard(guard: NavigationGuard) {
  active = guard;
  window.dispatchEvent(new Event('cma:protect-navigation'));
  return () => {
    if (active === guard) active = undefined;
  };
}

export function needsNavigationConfirmation(target: NavigationTarget) {
  return active?.leaves(target) === true;
}

export function requestNavigation(target: NavigationTarget, proceed: () => void) {
  if (active?.leaves(target)) active.confirm(proceed);
  else proceed();
}

/** Imperative links use the mounted router; standalone surfaces use native navigation. */
export function navigateAppPath(path: string) {
  const event = new CustomEvent<string>('cma:route-request', { detail: path, cancelable: true });
  if (window.dispatchEvent(event)) window.location.assign(path);
}
