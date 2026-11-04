import { useEffect, useState } from 'react';
import type { AgentPanel } from './AgentSidePanel';

export function useAgentPanel() {
  const [wide, setWide] = useState(() => window.matchMedia('(min-width: 1284px)').matches);
  const [selection, setPanel] = useState<AgentPanel | 'auto'>('auto');
  useEffect(() => {
    const query = window.matchMedia('(min-width: 1284px)');
    const changed = () => setWide(query.matches);
    query.addEventListener('change', changed);
    return () => query.removeEventListener('change', changed);
  }, []);
  const panel = selection === 'auto' ? (wide ? 'files' : undefined) : selection;
  return {
    panel,
    setPanel,
    detailsOpen: panel === 'details',
    setDetailsOpen: (open: boolean) => setPanel(open ? 'details' : undefined),
  };
}
