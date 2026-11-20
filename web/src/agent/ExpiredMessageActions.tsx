import { useState } from 'react';
import { Button } from '../ui/Button';
import { attachmentMarker } from './attachments';

export function ExpiredMessageActions({
  input,
  onRestore,
  disabled,
}: {
  readonly input?: string;
  readonly onRestore?: (text: string) => void;
  readonly disabled?: boolean;
}) {
  const [copyNotice, setCopyNotice] = useState('');
  const attachmentStart = input?.lastIndexOf(attachmentMarker) ?? -1;
  const text = attachmentStart < 0 ? input : input?.slice(0, attachmentStart);
  const readable = text !== undefined && text.trim() !== '';
  return (
    <>
      {attachmentStart >= 0 ? (
        <p>Attachments are not restored. Select the files again before sending.</p>
      ) : null}
      {readable ? (
        <>
          <div className="queued-input-outcomes__actions">
            {onRestore ? (
              <Button size="sm" disabled={disabled} onClick={() => onRestore(text)}>
                Restore to draft
              </Button>
            ) : null}
            <Button
              size="sm"
              onClick={async () => {
                try {
                  await navigator.clipboard.writeText(text);
                  setCopyNotice('Message text copied.');
                } catch {
                  setCopyNotice('Could not copy. Select the message text and copy it manually.');
                }
              }}
            >
              Copy message
            </Button>
          </div>
          <p>Nothing will be sent until you choose Send.</p>
        </>
      ) : (
        <p>Message text is unavailable. It cannot be restored.</p>
      )}
      {copyNotice ? <p role="status">{copyNotice}</p> : null}
    </>
  );
}
