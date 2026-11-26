import { useCallback, useState } from 'react';
import type { AgentPanel } from './AgentSidePanel';

export function useAgentPanel(viewerId: string, agentId: string | undefined) {
  const scope = JSON.stringify([viewerId, agentId]);
  const [selection, setSelection] = useState<{ scope: string; panel: AgentPanel }>({
    scope,
    panel: undefined,
  });
  if (selection.scope !== scope) setSelection({ scope, panel: undefined });
  const panel = selection.scope === scope ? selection.panel : undefined;
  const setPanel = useCallback(
    (panel: AgentPanel, expected?: AgentPanel) =>
      setSelection((current) =>
        current.scope === scope && (expected === undefined || current.panel === expected)
          ? { scope, panel }
          : current,
      ),
    [scope],
  );
  const setComputerOpen = useCallback(
    (open: boolean) =>
      setSelection((current) => {
        if (current.scope !== scope) return current;
        const computerOpen = current.panel !== undefined && current.panel !== 'details';
        if (computerOpen === open) return current;
        return { scope, panel: open ? 'browser' : undefined };
      }),
    [scope],
  );
  return {
    panel,
    setPanel,
    setComputerOpen,
    detailsOpen: panel === 'details',
    setDetailsOpen: (open: boolean) => setPanel(open ? 'details' : undefined),
  };
}
