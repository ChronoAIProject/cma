import { useState } from 'react';
import { DOCS_PATHS } from '../routing/docsRoutes';
import { BUILD_INFO } from '../shell/build';
import { Button } from '../ui/Button';

export interface CreationRequest {
  readonly title: string;
  readonly key: string;
  readonly startedAt: string;
}

export function CreationRequestHelp({
  request,
  status,
  refused,
}: {
  readonly request: Pick<CreationRequest, 'key' | 'startedAt'>;
  readonly status?: number;
  readonly refused: boolean;
}) {
  const [copy, setCopy] = useState<'idle' | 'copied' | 'failed'>('idle');
  const details = [
    'Document creation request',
    `Build: ${BUILD_INFO.copyText}`,
    `Request started (UTC): ${request.startedAt}`,
    `Creation request reference: ${request.key}`,
    `Last attempt: ${refused ? 'Refused' : 'Creation not confirmed'}`,
    ...(status === undefined ? [] : [`HTTP status: ${status}`]),
  ].join('\n');
  const copyDetails = async () => {
    try {
      await navigator.clipboard.writeText(details);
      setCopy('copied');
    } catch {
      setCopy('failed');
    }
  };
  return (
    <details className="creation-request-help">
      <summary>Get help with this request</summary>
      <p>These details exclude your document title, content and account information.</p>
      <pre tabIndex={0} aria-label="Request help details">
        {details}
      </pre>
      <Button onClick={() => void copyDetails()}>Copy request details</Button>
      <p role="status">
        {copy === 'copied'
          ? 'Request details copied.'
          : copy === 'failed'
            ? 'Could not copy. Select the details above and copy them manually.'
            : ''}
      </p>
      <a
        className="task-help-link"
        href={DOCS_PATHS['find-your-outputs']}
        target="_blank"
        rel="noopener noreferrer"
      >
        Open artifact help (new tab)
      </a>
    </details>
  );
}
