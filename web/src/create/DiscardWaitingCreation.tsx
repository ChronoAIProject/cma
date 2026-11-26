import { useState } from 'react';
import { Button } from '../ui/Button';
import { InlineConfirm } from '../ui/InlineConfirm';

export function DiscardWaitingCreation({
  label,
  remote = false,
  onDiscard,
}: {
  readonly label: 'task' | 'creation';
  readonly remote?: boolean;
  readonly onDiscard: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const action = remote ? 'Cancel setup' : `Discard ${label}`;
  return confirming ? (
    <InlineConfirm
      question={
        remote
          ? 'Cancel this Agent setup?'
          : `Discard this saved ${label} and start over in this tab?`
      }
      confirmLabel={action}
      onCancel={() => setConfirming(false)}
      onConfirm={onDiscard}
    />
  ) : (
    <Button variant="outline" onClick={() => setConfirming(true)}>
      {action}
    </Button>
  );
}
