import { CharacterAvatar } from '../character/CharacterAvatar';
import { Button } from '../ui/Button';
import type { RuntimeAttention } from './runtimeAttention';

export function RuntimeAttentionDisclosure({
  attention,
  selectedSandboxId,
  onSelect,
}: {
  readonly attention: RuntimeAttention;
  readonly selectedSandboxId?: string;
  readonly onSelect: (sandboxId: string) => void;
}) {
  if (attention.freshness === 'unknown' || attention.failedSandboxes.length === 0) return null;
  const fresh = attention.freshness === 'fresh';
  return (
    <details className="runtime-attention">
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
          </li>
        ))}
      </ul>
    </details>
  );
}
