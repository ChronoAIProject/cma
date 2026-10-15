import { useEffect, useRef, useState } from 'react';
import { Button } from '../ui/Button';
import { Presence } from '../ui/Presence';
import type { UploadAttachment } from './attachments';
import { MessageAvatar, messageIdentity, type MessageUser } from './MessageIdentity';
import { QueuedMessageEditor } from './QueuedMessageEditor';
import { UserMessage } from './UserMessage';
import './QueuedMessages.css';

export interface QueuedMessageItem {
  readonly responseId: string;
  readonly commandId: string;
  readonly revision: number;
  readonly input: string;
  readonly busy?: boolean;
  readonly error?: string;
  readonly needsRecovery?: boolean;
}
interface QueueActions {
  readonly disabled?: boolean;
  readonly onEdit: (item: QueuedMessageItem, input: string) => Promise<void>;
  readonly onRemove: (item: QueuedMessageItem) => Promise<void>;
  readonly onRecover?: (item: QueuedMessageItem) => Promise<void>;
  readonly uploadAttachment?: UploadAttachment;
  readonly user?: MessageUser;
}
const keyOf = (item: QueuedMessageItem) => `${item.responseId}:${item.commandId}`;
function retainExitingRows(
  previous: readonly QueuedMessageItem[],
  items: readonly QueuedMessageItem[],
) {
  const rows = [...items];
  previous.forEach((item, index) => {
    if (items.some((current) => keyOf(current) === keyOf(item))) return;
    const next = previous
      .slice(index + 1)
      .find((entry) => items.some((current) => keyOf(current) === keyOf(entry)));
    const position = next ? rows.findIndex((entry) => keyOf(entry) === keyOf(next)) : rows.length;
    rows.splice(position, 0, item);
  });
  return rows;
}

export function QueuedMessages({
  items,
  ...actions
}: QueueActions & { readonly items: readonly QueuedMessageItem[] }) {
  // Keep removed rows for Presence's inert exit frame; stable keys retain other drafts.
  const [stored, setStored] = useState({ source: items, rows: items });
  const current = new Map(items.map((item) => [keyOf(item), item]));
  if (stored.source !== items)
    setStored({
      source: items,
      rows: retainExitingRows(stored.rows, items),
    });
  useEffect(() => {
    const timer = window.setTimeout(
      () =>
        setStored((previous) =>
          previous.rows.some((item) => !items.some((current) => keyOf(current) === keyOf(item)))
            ? { source: items, rows: items }
            : previous,
        ),
      200,
    );
    return () => window.clearTimeout(timer);
  }, [items]);
  return (
    <Presence>
      {items.length ? (
        <section className="queued-messages" aria-label="Queued messages">
          <h3 className="queued-messages__heading">
            Queued <span>{items.length}</span>
          </h3>
          <ol className="queued-messages__list">
            {stored.rows.map((item) => (
              <Presence key={keyOf(item)}>
                {current.has(keyOf(item)) ? (
                  <li className="queued-messages__item">
                    <QueueRow
                      {...actions}
                      item={current.get(keyOf(item))!}
                      position={items.findIndex((entry) => keyOf(entry) === keyOf(item)) + 1}
                    />
                  </li>
                ) : null}
              </Presence>
            ))}
          </ol>
        </section>
      ) : null}
    </Presence>
  );
}

function QueueRow({
  item,
  position,
  disabled = false,
  onEdit,
  onRemove,
  onRecover,
  uploadAttachment,
  user,
}: QueueActions & { readonly item: QueuedMessageItem; readonly position: number }) {
  const [editing, setEditing] = useState<QueuedMessageItem>();
  const [editSequence, setEditSequence] = useState(0);
  const [working, setWorking] = useState<'remove' | 'recover'>();
  const [error, setError] = useState<string>();
  const active = useRef(false);
  const opener = useRef<HTMLDivElement>(null);
  const restoreFocus = useRef(false);
  const identity = messageIdentity('user', user);
  const busy = disabled || item.busy === true || working !== undefined;
  const locked = busy || item.needsRecovery === true;
  const close = () => {
    restoreFocus.current = true;
    setEditing(undefined);
  };
  useEffect(() => {
    if (!editing && restoreFocus.current) {
      restoreFocus.current = false;
      opener.current?.focus();
    }
  }, [editing]);
  const edit = () => {
    if (!locked) {
      setError(undefined);
      setEditSequence((sequence) => sequence + 1);
      setEditing(item);
    }
  };
  async function run(kind: 'remove' | 'recover') {
    if (active.current || busy || (kind === 'remove' && locked)) return;
    const action = kind === 'remove' ? onRemove : onRecover;
    if (!action) return;
    active.current = true;
    setWorking(kind);
    setError(undefined);
    try {
      await action(item);
      if (kind === 'recover') close();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : 'The request could not be confirmed.');
    } finally {
      active.current = false;
      setWorking(undefined);
    }
  }
  return (
    <article className="queued-message" aria-label={`Queued message ${String(position)}`}>
      <span className="queued-message__avatar">
        <MessageAvatar identity={identity} />
      </span>
      <div className="queued-message__body">
        <div className="queued-message__heading">
          <span className="queued-message__author">{identity.name}</span>
          <span className="queued-message__status">Queued</span>
        </div>
        <div
          className="queued-message__content"
          ref={opener}
          role="button"
          tabIndex={locked ? -1 : 0}
          aria-disabled={locked}
          aria-label="Edit queued message content"
          hidden={editing !== undefined}
          onClick={edit}
          onKeyDown={(event) => {
            if (event.key === 'Enter' || event.key === ' ') {
              event.preventDefault();
              edit();
            }
          }}
        >
          <UserMessage content={item.input} />
        </div>
        <Presence>
          {editing ? (
            <div className="queued-message__editor-frame">
              <QueuedMessageEditor
                key={editSequence}
                input={editing.input}
                disabled={locked}
                uploadAttachment={uploadAttachment}
                onCancel={close}
                onSave={async (input) => {
                  await onEdit(editing, input);
                  close();
                }}
              />
            </div>
          ) : null}
        </Presence>
        <div className="queued-message__actions">
          {editing ? null : (
            <Button size="sm" disabled={locked} onClick={edit}>
              Edit
            </Button>
          )}
          <Button
            size="sm"
            disabled={locked || editing !== undefined}
            onClick={() => void run('remove')}
          >
            {working === 'remove' ? 'Removing…' : 'Remove'}
          </Button>
          {item.needsRecovery && onRecover ? (
            <Button size="sm" disabled={busy} onClick={() => void run('recover')}>
              {working === 'recover' ? 'Recovering…' : 'Recover request'}
            </Button>
          ) : null}
        </div>
        {error || item.error ? (
          <p className="queued-messages__error" role="alert">
            {error ?? item.error}
          </p>
        ) : null}
      </div>
    </article>
  );
}
