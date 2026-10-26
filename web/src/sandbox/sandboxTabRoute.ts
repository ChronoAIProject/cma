import { routePath } from '../routing/routes';

/** The Sandbox page's tabs, in display order — `docs/Web_Style.md` §17. */
export const SANDBOX_TABS = ['overview', 'workspaces', 'access', 'activity'] as const;
export type SandboxTab = (typeof SANDBOX_TABS)[number];

function isSandboxTab(value: string | null): value is SandboxTab {
  return SANDBOX_TABS.some((tab) => tab === value);
}

/** The tab a URL names, or undefined when it names none or an unknown one. */
export function readSandboxTab(search: string): SandboxTab | undefined {
  const value = new URLSearchParams(search).get('tab');
  return isSandboxTab(value) ? value : undefined;
}

/** Overview is the page itself, so it carries no query. */
export function sandboxTabPath(sandboxId: string, tab: SandboxTab): string {
  const path = routePath({ kind: 'sandbox', sandboxId });
  return tab === 'overview' ? path : `${path}?tab=${tab}`;
}

/**
 * Write the selected tab onto the current history entry.
 *
 * Replacing rather than pushing keeps Back meaning "the previous page", not
 * "the previous tab". The write is skipped unless the address bar is still on
 * this Sandbox, so a late selection can never stamp a query onto another page.
 */
export function rememberSandboxTab(sandboxId: string, tab: SandboxTab): void {
  if (window.location.pathname !== routePath({ kind: 'sandbox', sandboxId })) return;
  window.history.replaceState(window.history.state, '', sandboxTabPath(sandboxId, tab));
}

/**
 * Navigate to a Sandbox and land on one of its tabs.
 *
 * Routes carry no query, so `navigate` pushes the bare Sandbox path. It does
 * so synchronously and the page reads its tab when it mounts, after this
 * returns, so the query written onto the entry just pushed is what it finds.
 */
export function openSandboxTab(navigate: () => void, sandboxId: string, tab: SandboxTab): void {
  navigate();
  rememberSandboxTab(sandboxId, tab);
}
