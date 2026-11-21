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
    (panel: AgentPanel) =>
      setSelection((current) => (current.scope === scope ? { scope, panel } : current)),
    [scope],
  );
  return {
    panel,
    setPanel,
    detailsOpen: panel === 'details',
    setDetailsOpen: (open: boolean) => setPanel(open ? 'details' : undefined),
  };
}
