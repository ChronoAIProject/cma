import { useEffect, useRef, useState } from 'react';
import {
  DocumentError,
  newDocumentShareToken,
  type DocumentsClient,
  type DocumentView,
  type DocumentPermission,
  type DocumentShare,
  type DocumentLink,
} from '../api/documents';
import type { CommandOptions } from '../api/client';
import { documentShareUrl } from './shareToken';
import { Modal } from '../ui/Modal';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Alert } from '../ui/Alert';
import { Picker } from '../ui/Picker';

type Intent = { key: string; action: (options: CommandOptions) => Promise<DocumentView> };
export function DocumentSharing({
  client,
  document,
  open,
  onUpdated,
  onClose,
}: {
  readonly client: DocumentsClient;
  readonly document: DocumentView;
  readonly open: boolean;
  readonly onUpdated: (document: DocumentView) => void;
  readonly onClose: () => void;
}) {
  const [shares, setShares] = useState<readonly DocumentShare[]>([]);
  const [links, setLinks] = useState<readonly DocumentLink[]>([]);
  const [urls, setUrls] = useState<ReadonlyMap<string, string>>(new Map());
  const [subject, setSubject] = useState('');
  const [permission, setPermission] = useState<DocumentPermission>('read');
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [reload, setReload] = useState(0);
  const [busy, setBusy] = useState(false);
  const [intent, setIntent] = useState<Intent>();
  const request = useRef<AbortController | undefined>(undefined);
  const locked = busy || intent !== undefined;
  useEffect(() => () => request.current?.abort(), []);
  useEffect(() => {
    if (!open) return;
    const read = new AbortController();
    void Promise.all([
      client.shares(document.id, { signal: read.signal }),
      client.links(document.id, { signal: read.signal }),
    ]).then(
      ([users, values]) => {
        if (!read.signal.aborted) {
          setShares(users);
          setLinks(values);
        }
      },
      (cause) => {
        if (!read.signal.aborted)
          setError(cause instanceof Error ? cause.message : 'Sharing could not be loaded.');
      },
    );
    return () => read.abort();
  }, [client, document.id, open, reload]);
  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(undefined), 3000);
    return () => clearTimeout(timer);
  }, [notice]);
  const run = async (action?: Intent['action']) => {
    if (request.current || document.access !== 'owner') return;
    const retained = intent ?? (action ? { key: crypto.randomUUID(), action } : undefined);
    if (!retained) return;
    const mutation = new AbortController();
    request.current = mutation;
    setIntent(retained);
    setBusy(true);
    setError(undefined);
    try {
      const next = await retained.action({ signal: mutation.signal, idempotencyKey: retained.key });
      if (mutation.signal.aborted) return;
      onUpdated(next);
      setIntent(undefined);
      setReload((value) => value + 1);
      setNotice('Sharing updated.');
    } catch (cause) {
      if (!mutation.signal.aborted) {
        if (cause instanceof DocumentError && cause.status >= 400 && cause.status < 500)
          setIntent(undefined);
        setError(
          cause instanceof Error
            ? cause.message
            : 'Sharing could not be confirmed. Retry the same change.',
        );
      }
    } finally {
      request.current = undefined;
      if (!mutation.signal.aborted) setBusy(false);
    }
  };
  const refresh = async () => {
    if (request.current) return;
    const read = new AbortController();
    request.current = read;
    setBusy(true);
    try {
      const next = await client.read(document.id, { signal: read.signal });
      if (read.signal.aborted) return;
      onUpdated(next);
      setError(undefined);
      setReload((value) => value + 1);
    } catch (cause) {
      if (!read.signal.aborted)
        setError(cause instanceof Error ? cause.message : 'Sharing could not be refreshed.');
    } finally {
      request.current = undefined;
      if (!read.signal.aborted) setBusy(false);
    }
  };
  const createLink = () => {
    const token = newDocumentShareToken();
    void run(async (options) => {
      const value = await client.createLink(
        document.id,
        { expected_revision: document.revision, permission, token },
        options,
      );
      if (value.link && !options.signal?.aborted)
        setUrls((old) => new Map(old).set(value.link!.id, documentShareUrl(document.id, token)));
      return value.document;
    });
  };
  return (
    <Modal
      open={open}
      title={`Share ${document.title}`}
      dismissible={!busy}
      onClose={onClose}
      footer={
        <Button onClick={onClose} disabled={busy}>
          Done
        </Button>
      }
    >
      <div className="document-sharing">
        <p>Share this document with a person or create a link. Only you can edit its contents.</p>
        <Picker
          label="Recipient permission"
          options={[
            { value: 'read', label: 'Can view' },
            { value: 'comment', label: 'Can comment' },
          ]}
          value={permission}
          disabled={locked}
          onChange={(value) => setPermission(value as DocumentPermission)}
        />
        <form
          className="document-sharing__add"
          onSubmit={(event) => {
            event.preventDefault();
            if (subject.trim())
              void run((options) =>
                client.grant(
                  document.id,
                  subject.trim(),
                  { expected_revision: document.revision, permission },
                  options,
                ),
              );
          }}
        >
          <label className="document-field">
            NyxID user ID
            <Input
              aria-label="Recipient user ID"
              value={subject}
              maxLength={255}
              disabled={locked}
              onChange={(event) => setSubject(event.target.value)}
            />
          </label>
          <Button type="submit" disabled={locked || !subject.trim()}>
            Add person
          </Button>
        </form>
        <ul className="document-sharing__list" aria-label="People with access">
          {shares.map((share) => (
            <li key={share.subject}>
              <span title={share.subject}>{share.subject}</span>
              <small>{share.permission === 'comment' ? 'Can comment' : 'Can view'}</small>
              <Button
                size="sm"
                disabled={locked}
                onClick={() =>
                  void run((options) =>
                    client.revoke(document.id, share.subject, document.revision, options),
                  )
                }
              >
                Remove
              </Button>
            </li>
          ))}
        </ul>
        <section>
          <h2>Share links</h2>
          <Button disabled={locked} onClick={createLink}>
            Create {permission === 'comment' ? 'comment' : 'view'} link
          </Button>
          <ul className="document-sharing__list" aria-label="Active share links">
            {links.map((link) => (
              <li key={link.id} className="document-sharing__link">
                <span>
                  {link.permission === 'comment' ? 'Can comment' : 'Can view'} ·{' '}
                  {new Date(link.created_at).toLocaleDateString()}
                </span>
                {urls.has(link.id) ? (
                  <>
                    <Input readOnly aria-label="Document share link" value={urls.get(link.id)} />
                    <Button
                      size="sm"
                      onClick={() =>
                        void navigator.clipboard.writeText(urls.get(link.id)!).then(
                          () => setNotice('Link copied.'),
                          () =>
                            setError(
                              'Copy the link from the field. Clipboard access is unavailable.',
                            ),
                        )
                      }
                    >
                      Copy link
                    </Button>
                  </>
                ) : (
                  <small>The original link is available only when created.</small>
                )}
                <Button
                  size="sm"
                  disabled={locked}
                  onClick={() =>
                    void run((options) =>
                      client.revokeLink(document.id, link.id, document.revision, options),
                    )
                  }
                >
                  Revoke
                </Button>
              </li>
            ))}
          </ul>
        </section>
        {notice ? <p role="status">{notice}</p> : null}
        {error ? (
          <Alert
            tone="error"
            actions={
              intent ? (
                <Button disabled={busy} onClick={() => void run()}>
                  Retry change
                </Button>
              ) : (
                <Button disabled={busy} onClick={() => void refresh()}>
                  Refresh access
                </Button>
              )
            }
          >
            {error}
          </Alert>
        ) : null}
      </div>
    </Modal>
  );
}
