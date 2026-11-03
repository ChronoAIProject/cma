import type { BrowserControlScope } from '@chronoai/cma-sdk';

export function controlRecovery(
  accountId: string,
  agentId: string,
  incarnation: string,
  contextId: string,
) {
  const key = `cma-browser-control:${JSON.stringify([accountId, agentId, incarnation, contextId])}`;
  const read = (): BrowserControlScope | undefined => {
    try {
      const saved = sessionStorage.getItem(key);
      if (!saved || saved.length > 2048) return;
      const value: unknown = JSON.parse(saved);
      if (!value || typeof value !== 'object') return;
      const record = value as Record<string, unknown>;
      if (
        record.runtime_incarnation !== incarnation ||
        record.context_id !== contextId ||
        typeof record.client_id !== 'string' ||
        record.client_id.length > 64 ||
        typeof record.client_key !== 'string' ||
        record.client_key.length !== 36 ||
        typeof record.operation_id !== 'string' ||
        record.operation_id.length > 64
      )
        return;
      return {
        runtime_incarnation: incarnation,
        context_id: contextId,
        client_id: record.client_id,
        client_key: record.client_key,
        operation_id: record.operation_id,
      };
    } catch {
      return;
    }
  };
  return {
    read,
    save(value: BrowserControlScope) {
      try {
        sessionStorage.setItem(
          key,
          JSON.stringify({
            runtime_incarnation: incarnation,
            context_id: contextId,
            client_id: value.client_id,
            client_key: value.client_key,
            operation_id: value.operation_id,
          }),
        );
      } catch {
        return;
      }
    },
    clear(operationId?: string) {
      try {
        if (read()?.operation_id === operationId) sessionStorage.removeItem(key);
      } catch {
        return;
      }
    },
  };
}
