import { useCallback, useEffect, useRef, useState } from 'react';
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
  const [editingKeys, setEditingKeys] = useState<ReadonlySet<string>>(new Set());
  const retainEditor = useCallback((key: string, editing: boolean) => {
    setEditingKeys((previous) => {
      const next = new Set(previous);
      if (editing) next.add(key);
      else next.delete(key);
      return next;
    });
  }, []);
  const current = new Map(items.map((item) => [keyOf(item), item]));
  const retained = stored.rows.filter(
    (item) => editingKeys.has(keyOf(item)) && !current.has(keyOf(item)),
  );
  if (stored.source !== items)
    setStored({
      source: items,
      rows: retainExitingRows(stored.rows, items),
    });
  useEffect(() => {
    const timer = window.setTimeout(
      () =>
        setStored((previous) =>
          previous.rows.some(
            (item) =>
              !editingKeys.has(keyOf(item)) &&
              !items.some((current) => keyOf(current) === keyOf(item)),
          )
            ? {
                source: items,
                rows: previous.rows.filter(
                  (item) =>
                    editingKeys.has(keyOf(item)) ||
                    items.some((current) => keyOf(current) === keyOf(item)),
                ),
              }
            : previous,
        ),
      200,
    );
    return () => window.clearTimeout(timer);
  }, [items, editingKeys]);
  return (
    <Presence>
      {items.length || retained.length ? (
        <section
          className="queued-messages"
          aria-label={items.length ? 'Queued messages' : 'Retained drafts'}
        >
          <h3 className="queued-messages__heading">
            {items.length ? 'Queued' : 'Retained drafts'}{' '}
            <span>{items.length || retained.length}</span>
          </h3>
          <ol className="queued-messages__list">
            {stored.rows.map((item) => (
              <Presence key={keyOf(item)}>
                {current.has(keyOf(item)) || editingKeys.has(keyOf(item)) ? (
                  <li className="queued-messages__item">
                    <QueueRow
                      {...actions}
                      item={current.get(keyOf(item)) ?? item}
                      unavailable={!current.has(keyOf(item))}
                      position={
                        (current.has(keyOf(item)) ? items : retained).findIndex(
                          (entry) => keyOf(entry) === keyOf(item),
                        ) + 1
                      }
                      onEditingChange={(editing) => retainEditor(keyOf(item), editing)}
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
  unavailable,
  onEditingChange,
  disabled = false,
  onEdit,
  onRemove,
  onRecover,
  uploadAttachment,
  user,
}: QueueActions & {
  readonly item: QueuedMessageItem;
  readonly position: number;
  readonly unavailable: boolean;
  readonly onEditingChange: (editing: boolean) => void;
}) {
  const [editing, setEditing] = useState<QueuedMessageItem>();
  const [editSequence, setEditSequence] = useState(0);
  const [working, setWorking] = useState<'remove' | 'recover'>();
  const [error, setError] = useState<string>();
  const active = useRef(false);
  const opener = useRef<HTMLDivElement>(null);
  const restoreFocus = useRef(false);
  const identity = messageIdentity('user', user);
  const busy = disabled || (!unavailable && item.busy === true) || working !== undefined;
  const locked = busy || item.needsRecovery === true || unavailable;
  const close = () => {
    restoreFocus.current = true;
    setEditing(undefined);
    onEditingChange(false);
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
      onEditingChange(true);
    }
  };
  async function run(kind: 'remove' | 'recover') {
    if (unavailable || active.current || busy || (kind === 'remove' && locked)) return;
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
    <article
      className="queued-message"
      aria-label={`${unavailable ? 'Retained message draft' : 'Queued message'} ${String(position)}`}
    >
      <span className="queued-message__avatar">
        <MessageAvatar identity={identity} />
      </span>
      <div className="queued-message__body">
        <div className="queued-message__heading">
          <span className="queued-message__author">{identity.name}</span>
          <span className="queued-message__status">
            {unavailable ? 'Draft retained' : 'Queued'}
          </span>
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
                disabled={busy || (!unavailable && item.needsRecovery === true)}
                unavailable={unavailable}
                uploadAttachment={uploadAttachment}
                onCancel={close}
                onSave={async (input) => {
                  if (unavailable) throw new Error('This message is no longer queued.');
                  await onEdit(editing, input);
                  close();
                }}
              />
            </div>
          ) : null}
        </Presence>
        {!unavailable ? (
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
        ) : null}
        {error || item.error ? (
          <p className="queued-messages__error" role="alert">
            {error ?? item.error}
          </p>
        ) : null}
      </div>
    </article>
  );
}
