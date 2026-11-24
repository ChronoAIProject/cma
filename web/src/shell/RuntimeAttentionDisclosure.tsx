import { useLayoutEffect, useRef, type ReactNode } from 'react';
import { CharacterAvatar } from '../character/CharacterAvatar';
import { Button } from '../ui/Button';
import type { RuntimeAttention } from './runtimeAttention';

export function RuntimeAttentionDisclosure({
  attention,
  selectedSandboxId,
  selectedAgentId,
  revealAgents = false,
  renderAgents,
  onSelect,
}: {
  readonly attention: RuntimeAttention;
  readonly selectedSandboxId?: string;
  readonly selectedAgentId?: string;
  readonly revealAgents?: boolean;
  readonly renderAgents?: (sandboxId: string) => ReactNode;
  readonly onSelect: (sandboxId: string) => void;
}) {
  const disclosure = useRef<HTMLDetailsElement>(null);
  const selectedFailure = attention.failedSandboxes.some(
    (sandbox) =>
      sandbox.sandboxId === selectedSandboxId ||
      (selectedAgentId !== undefined && sandbox.agentIds.includes(selectedAgentId)),
  );
  const visible = attention.freshness !== 'unknown' && attention.failedSandboxes.length > 0;
  useLayoutEffect(() => {
    if (visible && disclosure.current && (revealAgents || selectedFailure))
      disclosure.current.open = true;
  }, [revealAgents, selectedAgentId, selectedFailure, selectedSandboxId, visible]);
  if (!visible) return null;
  const fresh = attention.freshness === 'fresh';
  return (
    <details ref={disclosure} className="runtime-attention">
      <summary>
        <CharacterAvatar pose="failed" tone="error" still size={24} />
        <span>
          {attention.failedSandboxes.length} updates failed{fresh ? '' : ' (last known)'} · Fix
        </span>
      </summary>
      {!fresh ? (
        <p>Environment status could not be confirmed. These are last-known updates.</p>
      ) : null}
      <ul aria-label="Environments with failed updates">
        {attention.failedSandboxes.map((sandbox) => (
          <li key={sandbox.sandboxId}>
            <Button
              variant="ghost"
              aria-current={sandbox.sandboxId === selectedSandboxId ? 'page' : undefined}
              onClick={() => onSelect(sandbox.sandboxId)}
            >
              {sandbox.name.trim() || 'Environment'}
            </Button>
            <code>{sandbox.sandboxId}</code>
            {renderAgents?.(sandbox.sandboxId)}
          </li>
        ))}
      </ul>
    </details>
  );
}
