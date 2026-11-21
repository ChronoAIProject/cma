import { useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { AgentProvenanceInventoryItem, AgentProvenanceInventoryOptions } from '@cma/client';
import type { ResourceViewsClient } from './client';
import { Button } from '../ui/Button';

export function inventoryExport(rows: readonly AgentProvenanceInventoryItem[]): string {
  return (
    JSON.stringify(
      {
        agents: rows.map((row) => ({
          agent_id: row.agent_id,
          title: row.title,
          status: row.status,
          created_at: row.created_at,
          updated_at: row.updated_at,
          creation_origin: row.creation_origin,
          ...(row.channel_usage === undefined
            ? {}
            : {
                channel_usage: {
                  active: row.channel_usage.active,
                  has_history: row.channel_usage.has_history,
                  first_used_at: row.channel_usage.first_used_at,
                  last_used_at: row.channel_usage.last_used_at,
                },
              }),
        })),
      },
      null,
      2,
    ) + '\n'
  );
}

function download(text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: 'application/json' }));
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = 'cma-agents-page.json';
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

export function ExportInventoryPage({
  client,
  options,
  disabled,
  onAccessChanged,
}: {
  readonly client: ResourceViewsClient;
  readonly options: AgentProvenanceInventoryOptions;
  readonly disabled: boolean;
  readonly onAccessChanged?: () => void;
}) {
  const context = useMemo(() => ({ client, options }), [client, options]);
  const current = useRef(context);
  const pending = useRef<
    { readonly context: typeof context; readonly request: AbortController } | undefined
  >(undefined);
  const [state, setState] = useState<{
    readonly context: typeof context;
    readonly status: 'loading' | 'error' | 'done';
    readonly message?: string;
  }>();
  useLayoutEffect(() => {
    current.current = context;
    return () => {
      if (pending.current?.context === context) pending.current.request.abort();
    };
  }, [context]);
  const status = state?.context === context ? state : undefined;
  const run = async () => {
    pending.current?.request.abort();
    const request = new AbortController();
    pending.current = { context, request };
    setState({ context, status: 'loading' });
    const timer = setTimeout(() => request.abort(), 15_000);
    try {
      // Re-read this page under current authorization before creating the file.
      const page = await context.client.agents.provenanceInventory({
        ...context.options,
        signal: request.signal,
      });
      request.signal.throwIfAborted();
      if (current.current !== context) return;
      download(inventoryExport(page.items));
      setState({ context, status: 'done' });
    } catch (error) {
      if (current.current === context) {
        setState({
          context,
          status: 'error',
          message:
            error instanceof Error && !request.signal.aborted
              ? error.message
              : 'The export could not check current access. Try again.',
        });
        if (!request.signal.aborted) onAccessChanged?.();
      }
    } finally {
      clearTimeout(timer);
    }
  };
  return (
    <div className="agent-inventory__export">
      <Button disabled={disabled || status?.status === 'loading'} onClick={() => void run()}>
        Export this page
      </Button>
      {status?.status === 'loading' ? <span role="status">Checking access for export</span> : null}
      {status?.status === 'error' ? <span role="alert">{status.message}</span> : null}
      {status?.status === 'done' ? <span role="status">Page exported</span> : null}
    </div>
  );
}
