import { useEffect, useId, useRef, useState } from 'react';
import { Button } from '../ui/Button';
import { IconCopyOutline16, IconRefreshOutline16, IconWarningOutline16 } from '../ui/icons';
import './DocumentSaveNotice.css';

interface DocumentSaveNoticeProps {
  readonly text: string;
  readonly busy: boolean;
  readonly canRetry: boolean;
  readonly error?: string;
  readonly onRetry: () => void;
}

export function DocumentSaveNotice({
  text,
  busy,
  canRetry,
  error,
  onRetry,
}: DocumentSaveNoticeProps) {
  const titleId = useId();
  const copyId = useId();
  const attempt = useRef(0);
  const [copy, setCopy] = useState<{
    readonly text: string;
    readonly state: 'copying' | 'copied' | 'failed';
  }>();
  useEffect(
    () => () => {
      attempt.current++;
    },
    [],
  );
  const copyText = async () => {
    if (copy?.state === 'copying') return;
    const current = ++attempt.current;
    setCopy({ text, state: 'copying' });
    try {
      if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable');
      await navigator.clipboard.writeText(text);
      if (attempt.current === current) setCopy({ text, state: 'copied' });
    } catch {
      if (attempt.current === current) setCopy({ text, state: 'failed' });
    }
  };
  const copied = copy?.text === text && copy.state === 'copied';
  return (
    <section className="document-save-notice" aria-labelledby={titleId}>
      <div className="document-save-notice__message" role={busy ? undefined : 'alert'}>
        <h2 id={titleId}>
          <IconWarningOutline16 />
          {busy ? 'Saving your document…' : 'We can’t confirm your last save'}
        </h2>
        <p>
          {busy
            ? 'Your edits are still on this page. Waiting for confirmation.'
            : 'Your edits are still on this page. Retry the same save to resolve its outcome, or copy the text before leaving.'}
        </p>
        {error && !busy ? (
          <details>
            <summary>Save details</summary>
            <p>{error}</p>
          </details>
        ) : null}
      </div>
      <div className="document-save-notice__actions">
        <Button
          variant="primary"
          icon={<IconRefreshOutline16 />}
          disabled={busy || !canRetry}
          onClick={onRetry}
        >
          Retry same save
        </Button>
        <Button
          variant="outline"
          icon={<IconCopyOutline16 />}
          disabled={copy?.state === 'copying'}
          onClick={() => void copyText()}
        >
          Copy all text
        </Button>
      </div>
      <div className="document-save-notice__copy" aria-live="polite">
        {copied ? <p>All text copied.</p> : null}
        {copy?.state === 'failed' ? (
          <>
            <label htmlFor={copyId}>Copy failed. Select and copy the text below.</label>
            <textarea
              id={copyId}
              aria-label="Document text to copy"
              readOnly
              value={text}
              onFocus={(event) => event.currentTarget.select()}
            />
          </>
        ) : null}
      </div>
    </section>
  );
}
