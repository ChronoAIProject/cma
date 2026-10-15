import { useEffect, useRef } from 'react';
import { Button } from '../ui/Button';
import { IconCloseOutline16, IconPaperclipOutline16 } from '../ui/icons';
import type { DraftAttachment } from './attachments';
import './ComposerAttachments.css';

function Preview({ file }: { readonly file: File }) {
  const image = useRef<HTMLImageElement>(null);
  const supported = /^image\/(png|jpeg|gif|webp|avif)$/.test(file.type);
  useEffect(() => {
    if (!supported || image.current === null) return;
    const value = URL.createObjectURL(file);
    image.current.src = value;
    return () => URL.revokeObjectURL(value);
  }, [file, supported]);
  return supported ? <img ref={image} alt="" /> : <IconPaperclipOutline16 />;
}

export function ComposerAttachments({
  items,
  error,
  enabled,
  locked,
  add,
  remove,
  retry,
}: {
  readonly items: readonly DraftAttachment[];
  readonly error?: string;
  readonly enabled: boolean;
  readonly locked: boolean;
  readonly add: (files: readonly File[]) => void;
  readonly remove: (key: string) => void;
  readonly retry: (key: string) => void;
}) {
  const picker = useRef<HTMLInputElement>(null);
  return (
    <div className="composer-attachments">
      {items.length === 0 ? null : (
        <ul className="composer-attachments__list" aria-label="Attachments">
          {items.map((item) => (
            <li className="composer-attachments__item" key={item.key}>
              <span className="composer-attachments__preview">
                <Preview file={item.file} />
              </span>
              <div className="composer-attachments__info">
                <span title={item.file.name}>{item.file.name}</span>
                <small role="status" title={item.error}>
                  {item.status === 'ready'
                    ? 'Ready'
                    : item.status === 'error'
                      ? item.error
                      : item.status === 'uploading'
                        ? 'Uploading…'
                        : 'Queued'}
                </small>
              </div>
              {item.status === 'error' ? (
                <Button size="sm" disabled={!enabled || locked} onClick={() => retry(item.key)}>
                  Retry
                </Button>
              ) : null}
              <Button
                size="sm"
                icon={<IconCloseOutline16 />}
                disabled={locked}
                aria-label={`Remove ${item.file.name}`}
                onClick={() => remove(item.key)}
              />
            </li>
          ))}
        </ul>
      )}
      {error === undefined ? null : (
        <p className="composer-attachments__error" role="alert">
          {error}
        </p>
      )}
      <div className="composer-attachments__toolbar">
        <input
          ref={picker}
          type="file"
          multiple
          className="sr-only"
          tabIndex={-1}
          aria-label="Choose attachments"
          disabled={!enabled || locked}
          onChange={(event) => {
            add(Array.from(event.target.files ?? []));
            event.target.value = '';
          }}
        />
        <Button
          size="sm"
          icon={<IconPaperclipOutline16 />}
          disabled={!enabled || locked}
          aria-label="Attach files"
          title="Attach files (up to 20 MiB each)"
          onClick={() => picker.current?.click()}
        />
        <small>Paste images or drop files · 20 MiB each</small>
      </div>
    </div>
  );
}
