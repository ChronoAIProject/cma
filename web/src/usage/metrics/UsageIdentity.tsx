import './UsageIdentity.css';
import { useState } from 'react';
import { Button } from '../../ui/Button';
import { IconCopyOutline16 } from '../../ui/icons/IconCopyOutline16';

export function compactIdentity(value: string): string {
  return value.length > 26 ? `${value.slice(0, 14)}…${value.slice(-8)}` : value;
}

export function UsageIdentity({
  value,
  label,
}: {
  readonly value: string;
  readonly label: string;
}) {
  const [feedback, setFeedback] = useState('');
  return (
    <div className="usage-identity">
      <code title={value} tabIndex={0} aria-label={`${label}: ${value}`}>
        {compactIdentity(value)}
      </code>
      <Button
        size="sm"
        icon={<IconCopyOutline16 />}
        aria-label={`Copy ${label}`}
        title={`Copy ${label}`}
        onClick={() => {
          void Promise.resolve()
            .then(() => navigator.clipboard.writeText(value))
            .then(
              () => setFeedback(`${label} copied`),
              () => setFeedback('Copy unavailable. Select the full value below.'),
            );
        }}
      />
      {feedback && (
        <span className="usage-identity__feedback" role="status">
          {feedback}
        </span>
      )}
      {feedback.startsWith('Copy unavailable') && (
        <code className="usage-identity__full">{value}</code>
      )}
    </div>
  );
}
