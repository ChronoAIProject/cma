import { useState } from 'react';
import { Button } from '../ui/Button';
import { InlineConfirm } from '../ui/InlineConfirm';

export function DiscardWaitingCreation({
  label,
  onDiscard,
}: {
  readonly label: 'task' | 'creation';
  readonly onDiscard: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const action = `Discard ${label}`;
  return confirming ? (
    <InlineConfirm
      question={`Discard this saved ${label} and start over in this tab?`}
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
