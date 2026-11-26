import type { AgentLifecycleCreateInput, AgentLifecycleCreateReceipt } from '../api/client';
import { parseTaskTemplate } from './taskTemplateModel';
import { requireTaskStorage } from './taskIdentity';

export interface Intent {
  readonly draftRevision?: string;
  readonly key: string;
  readonly body: AgentLifecycleCreateInput;
  readonly receipt?: AgentLifecycleCreateReceipt;
  readonly agentId?: string;
}
export interface AgentBinding {
  readonly agentId: string;
  readonly operationId: string;
}
export interface IntentScope {
  readonly owner?: string;
  readonly workspaceId?: string;
  readonly sandboxId?: string;
}
export function lifecycleStorageKey(owner: string, workspaceId?: string, sandboxId?: string) {
  return `cma:agent-creation:${JSON.stringify(sandboxId !== undefined ? [location.origin, owner, 'sandbox', sandboxId] : workspaceId === undefined ? [location.origin, owner] : [location.origin, owner, workspaceId])}`;
}
export function lifecycleRecoveryKey(scope: IntentScope, binding: AgentBinding) {
  return `cma:agent-recovery:${JSON.stringify([location.origin, scope.owner, scope.workspaceId ?? null, scope.sandboxId ?? null, binding.agentId, binding.operationId])}`;
}
function pendingKey(scope: IntentScope) {
  requireTaskStorage();
  if (!scope.owner) throw new Error('The request owner could not be verified.');
  return lifecycleStorageKey(scope.owner, scope.workspaceId, scope.sandboxId);
}
function parse(raw: string, { workspaceId, sandboxId }: IntentScope): Intent {
  const saved = JSON.parse(raw) as { version: number; intent: Intent };
  const intent = saved?.intent;
  if (
    saved.version !== 1 ||
    !intent ||
    (intent.draftRevision !== undefined &&
      (typeof intent.draftRevision !== 'string' ||
        !/^[a-zA-Z0-9-]{1,200}$/.test(intent.draftRevision))) ||
    typeof intent.key !== 'string' ||
    !/^[a-zA-Z0-9-]{1,200}$/.test(intent.key) ||
    typeof intent.body?.name !== 'string' ||
    !intent.body.name ||
    (intent.body.title_mode !== undefined &&
      intent.body.title_mode !== 'automatic' &&
      intent.body.title_mode !== 'named') ||
    (intent.body.initial_message !== undefined &&
      typeof intent.body.initial_message !== 'string') ||
    (intent.body.desktop !== undefined && typeof intent.body.desktop !== 'boolean') ||
    (intent.body.machine_included !== undefined &&
      typeof intent.body.machine_included !== 'boolean') ||
    (intent.body.repository !== undefined &&
      (!intent.body.repository ||
        typeof intent.body.repository.url !== 'string' ||
        (intent.body.repository.ref !== undefined &&
          typeof intent.body.repository.ref !== 'string'))) ||
    intent.body.workspace_id !== workspaceId ||
    intent.body.sandbox_id !== sandboxId ||
    (intent.agentId !== undefined && (typeof intent.agentId !== 'string' || !intent.agentId)) ||
    (intent.receipt !== undefined && (typeof intent.receipt.id !== 'string' || !intent.receipt.id))
  )
    throw new Error();
  if (intent.body.template !== undefined) parseTaskTemplate(intent.body.template);
  return intent;
}
function encode(intent: Intent) {
  return JSON.stringify({ version: 1, intent });
}
export function hasPendingLifecycleIntent(scope: IntentScope) {
  try {
    return sessionStorage.getItem(pendingKey(scope)) !== null;
  } catch {
    return true;
  }
}
function same(left: Intent | undefined, right: Intent | undefined) {
  return JSON.stringify(left) === JSON.stringify(right);
}
function bindingFor(intent: Intent): AgentBinding {
  if (!intent.agentId || !intent.receipt) throw new Error('Agent binding is incomplete.');
  return { agentId: intent.agentId, operationId: intent.receipt.id };
}
function read(key: string, scope: IntentScope) {
  const raw = sessionStorage.getItem(key);
  return raw === null ? undefined : parse(raw, scope);
}
function compare(key: string, scope: IntentScope, expected: Intent | undefined) {
  if (!same(read(key, scope), expected))
    throw new Error('The saved request changed. Reopen this page to recover it.');
}
function archive(scope: IntentScope, intent: Intent) {
  const key = lifecycleRecoveryKey(scope, bindingFor(intent));
  const existing = read(key, scope);
  if (existing && !same(existing, intent)) throw new Error('Agent recovery identity changed.');
  if (!existing) sessionStorage.setItem(key, encode(intent));
  compare(key, scope, intent);
}

/** Archive first; a failed write or removal leaves the original request recoverable. */
function migrate(scope: IntentScope) {
  const key = pendingKey(scope);
  const intent = read(key, scope);
  if (intent?.agentId) {
    archive(scope, intent);
    compare(key, scope, intent);
    sessionStorage.removeItem(key);
    compare(key, scope, undefined);
    return undefined;
  }
  return intent;
}
export function loadLifecycleIntent(
  scope: IntentScope,
  binding?: AgentBinding,
): {
  intent?: Intent;
  error?: string;
} {
  try {
    pendingKey(scope);
    if (!binding) return { intent: migrate(scope) };
    let intent = read(lifecycleRecoveryKey(scope, binding), scope);
    // A new request in the pending slot must never obstruct an older Agent's recovery.
    if (!intent) {
      const legacy = read(pendingKey(scope), scope);
      if (legacy?.agentId === binding.agentId && legacy.receipt?.id === binding.operationId) {
        migrate(scope);
        intent = read(lifecycleRecoveryKey(scope, binding), scope);
      }
    }
    if (
      intent &&
      (intent.agentId !== binding.agentId || intent.receipt?.id !== binding.operationId)
    )
      throw new Error('Agent recovery identity changed.');
    return { intent };
  } catch {
    return {
      error:
        'The saved Agent request could not be read. Copy your task and restore browser session storage before continuing.',
    };
  }
}

/** Every writer compares the request it observed, including its body, receipt and binding. */
export function retainLifecycleIntent(
  scope: IntentScope,
  expected: Intent | undefined,
  next?: Intent,
  beforeRemove?: () => void,
) {
  const pending = pendingKey(scope);
  const key = expected?.agentId ? lifecycleRecoveryKey(scope, bindingFor(expected)) : pending;
  compare(key, scope, expected);
  if (next) {
    parse(encode(next), scope);
    if (
      expected &&
      (next.key !== expected.key ||
        !same({ ...expected, receipt: next.receipt, agentId: next.agentId }, next) ||
        (expected.receipt && expected.receipt.id !== next.receipt?.id) ||
        (expected.agentId && expected.agentId !== next.agentId))
    )
      throw new Error('Agent request identity changed.');
    if (next.agentId && !expected?.agentId) {
      archive(scope, next);
      compare(key, scope, expected);
      sessionStorage.removeItem(key);
      compare(key, scope, undefined);
    } else {
      sessionStorage.setItem(key, encode(next));
      compare(key, scope, next);
    }
  } else {
    beforeRemove?.();
    sessionStorage.removeItem(key);
    compare(key, scope, undefined);
  }
}

/** Find a recovery record only in this origin and owner's exact context. */
export function lifecycleScopeForAgent(owner: string, agentId: string) {
  try {
    requireTaskStorage();
    const keys = Array.from({ length: sessionStorage.length }, (_, index) =>
      sessionStorage.key(index),
    );
    const found = new Map<string, IntentScope & AgentBinding>();
    for (const key of keys) {
      if (
        !key ||
        (!key.startsWith('cma:agent-creation:') && !key.startsWith('cma:agent-recovery:'))
      )
        continue;
      let value: Intent;
      try {
        const raw = sessionStorage.getItem(key);
        if (!raw) continue;
        const body = (JSON.parse(raw) as { intent?: Intent }).intent?.body;
        if (!body) continue;
        const scope = { owner, workspaceId: body.workspace_id, sandboxId: body.sandbox_id };
        value = parse(raw, scope);
        if (value.agentId !== agentId || !value.receipt) continue;
        const binding = bindingFor(value);
        const journal = lifecycleRecoveryKey(scope, binding);
        if (key !== pendingKey(scope) && key !== journal) continue;
        found.set(journal, { ...scope, ...binding });
      } catch {
        continue;
      }
    }
    const value = found.size === 1 ? [...found.values()][0] : undefined;
    return (
      value && {
        workspaceId: value.workspaceId,
        sandboxId: value.sandboxId,
        operationId: value.operationId,
      }
    );
  } catch {
    return undefined;
  }
}
