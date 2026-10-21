import { Picker } from '../ui/Picker';
import { Presence } from '../ui/Presence';
import { useEffect, useRef, useState } from 'react';
import type { TriggerDocument } from '@chronoai/cma-sdk';
import type { AppRoute } from '../routing/routes';
import { Button } from '../ui/Button';
import { PageHeader } from '../ui/PageHeader';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import {
  IconPlusOutline16,
  IconRefreshOutline16,
  IconEditOutline16,
  IconCodeOutline16,
} from '../ui/icons';
import type { TriggerPublisherClient } from './client';
import { EmbedPanel } from './EmbedPanel';
import { TriggerWizard } from './TriggerWizard';
import './Publisher.css';
import '../profiles/Profiles.css';

export function TriggerPublisherPage({
  client,
  triggerId,
  view = 'wizard',
  onNavigate,
}: {
  readonly client: TriggerPublisherClient;
  readonly triggerId?: string;
  readonly view?: 'wizard' | 'embed';
  readonly onNavigate: (route: AppRoute) => void;
}) {
  const [items, setItems] = useState<readonly TriggerDocument[]>([]);
  const [next, setNext] = useState<string>();
  const [listLoading, setListLoading] = useState(true);
  const [listFailure, setListFailure] = useState(false);
  const [readResult, setReadResult] = useState<{
    readonly key: string;
    readonly document?: TriggerDocument;
    readonly failure?: boolean;
  }>();
  const [reload, setReload] = useState(0);
  const [listReload, setListReload] = useState(0);
  const [newVersion, setNewVersion] = useState(0);
  const [initialStep, setInitialStep] = useState(0);
  const [locked, setLocked] = useState(false);
  const pagination = useRef<AbortController | undefined>(undefined);
  const requestKey = `${triggerId ?? 'new'}:${String(reload)}`;
  const document = readResult?.key === requestKey ? readResult.document : undefined;
  const loading = triggerId !== undefined && readResult?.key !== requestKey;
  const failure = readResult?.key === requestKey && readResult.failure === true;
  useEffect(() => {
    const request = new AbortController();
    void client.triggers
      .list({ signal: request.signal })
      .then((page) => {
        if (!request.signal.aborted) {
          setItems(page.items);
          setNext(page.next);
          setListLoading(false);
        }
      })
      .catch(() => {
        if (!request.signal.aborted) {
          setListFailure(true);
          setListLoading(false);
        }
      });
    return () => {
      request.abort();
      pagination.current?.abort();
    };
  }, [client, listReload]);
  useEffect(() => {
    const request = new AbortController();
    if (triggerId === undefined) {
      return () => request.abort();
    }
    void client.triggers
      .read(triggerId, { signal: request.signal })
      .then((value) => {
        if (!request.signal.aborted) {
          setReadResult({ key: requestKey, document: value });
        }
      })
      .catch(() => {
        if (!request.signal.aborted) {
          setReadResult({ key: requestKey, failure: true });
        }
      });
    return () => request.abort();
  }, [client, triggerId, requestKey]);
  const updateDocument = (value: TriggerDocument) => {
    setReadResult({ key: requestKey, document: value });
    setItems((current) => [value, ...current.filter((item) => item.id !== value.id)]);
  };
  const navigate = (id: string) => {
    if (locked) return;
    setInitialStep(0);
    onNavigate(
      id === ''
        ? { kind: 'triggers' }
        : {
            kind:
              (items.find((item) => item.id === id)?.revision_count ?? 0) > 0
                ? 'trigger_embed'
                : 'trigger',
            triggerId: id,
          },
    );
  };
  const more = async () => {
    if (next === undefined || listLoading) return;
    const cursor = next;
    const request = new AbortController();
    pagination.current = request;
    setListLoading(true);
    try {
      const page = await client.triggers.list({ cursor, signal: request.signal });
      if (request.signal.aborted) return;
      if (page.next === cursor) throw new Error('Pagination did not advance.');
      setItems((current) => [
        ...current,
        ...page.items.filter((item) => !current.some((old) => old.id === item.id)),
      ]);
      setNext(page.next);
    } catch {
      if (!request.signal.aborted) setListFailure(true);
    } finally {
      if (!request.signal.aborted) setListLoading(false);
    }
  };
  const selectedId = document?.id ?? triggerId ?? '';
  const currentTitle =
    items.find((item) => item.id === selectedId)?.draft.presentation.title ??
    (selectedId === '' ? 'New Trigger' : 'Trigger');
  useDocumentTitle(selectedId === '' ? 'Triggers' : currentTitle);
  return (
    <section className="trigger-publisher" aria-labelledby="trigger-publisher-title">
      <div className="trigger-publisher__header">
        <PageHeader
          breadcrumb={[
            {
              label: 'Triggers',
              href: '/triggers/manage',
              onSelect: locked ? undefined : () => navigate(''),
            },
            { label: currentTitle },
          ]}
          title="Triggers"
          headingId="trigger-publisher-title"
          description="A Trigger is a button that brings each visitor to an Agent of their own."
          actions={
            <Button
              variant="primary"
              icon={<IconPlusOutline16 />}
              disabled={locked}
              onClick={() => {
                setReadResult(undefined);
                setNewVersion((value) => value + 1);
                navigate('');
              }}
            >
              New Trigger
            </Button>
          }
        />
      </div>
      <div className="trigger-publisher__body">
        <nav className="trigger-rail" aria-label="Your Triggers">
          <label className="trigger-rail__mobile">
            <span>Your Triggers</span>
            <Picker
              label="Your Triggers"
              value={selectedId}
              disabled={locked}
              onChange={(choice) => navigate(choice)}
              options={[
                { value: '', label: 'New Trigger' },
                ...items.map((item) => ({
                  value: item.id,
                  label: `${item.draft.presentation.title} / ${item.availability}`,
                })),
              ]}
            />
          </label>
          <ul>
            {items.map((item) => (
              <li key={item.id}>
                <a
                  href={`/triggers/manage/${encodeURIComponent(item.id)}${item.revision_count > 0 ? '/embed' : ''}`}
                  aria-current={selectedId === item.id ? 'page' : undefined}
                  aria-disabled={locked}
                  onClick={(event) => {
                    if (locked) {
                      event.preventDefault();
                      return;
                    }
                    if (
                      event.button === 0 &&
                      !event.metaKey &&
                      !event.ctrlKey &&
                      !event.shiftKey &&
                      !event.altKey
                    ) {
                      event.preventDefault();
                      navigate(item.id);
                    }
                  }}
                >
                  <strong>{item.draft.presentation.title}</strong>
                  <span>{item.availability}</span>
                  <code>{item.public_id?.slice(0, 12) ?? 'Not published'}</code>
                </a>
              </li>
            ))}
          </ul>
          {listLoading ? <p role="status">Loading Triggers</p> : null}
          <Presence>
            {listFailure ? (
              <div role="alert">
                <p>Triggers could not be loaded.</p>
                <Button
                  icon={<IconRefreshOutline16 />}
                  disabled={listLoading}
                  onClick={() => {
                    setListLoading(true);
                    setListFailure(false);
                    setListReload((value) => value + 1);
                  }}
                >
                  Try again
                </Button>
              </div>
            ) : null}
          </Presence>
          {next === undefined ? null : (
            <Button disabled={listLoading} onClick={() => void more()}>
              Load more
            </Button>
          )}
        </nav>
        <div className="trigger-publisher__editor">
          {view !== 'embed' && document !== undefined && document.revision_count > 0 ? (
            <div className="trigger-section-heading">
              <Button
                icon={<IconCodeOutline16 />}
                disabled={locked}
                onClick={() => onNavigate({ kind: 'trigger_embed', triggerId: document.id })}
              >
                Get embed code
              </Button>
            </div>
          ) : null}
          <>
            {loading ? (
              <p role="status">Loading Trigger</p>
            ) : failure ? null : view === 'embed' && document !== undefined ? (
              <>
                <EmbedPanel
                  client={client}
                  document={document}
                  onManageSources={() => {
                    setInitialStep(3);
                    onNavigate({ kind: 'trigger', triggerId: document.id });
                  }}
                />
                <Button
                  icon={<IconEditOutline16 />}
                  onClick={() => {
                    setInitialStep(0);
                    onNavigate({ kind: 'trigger', triggerId: document.id });
                  }}
                >
                  Edit Trigger
                </Button>
              </>
            ) : (
              <TriggerWizard
                key={triggerId ?? `new:${String(newVersion)}`}
                client={client}
                document={document}
                initialStep={initialStep}
                onChange={updateDocument}
                onLock={setLocked}
                onSaved={(value, savedStep) => {
                  if (triggerId === undefined) {
                    setInitialStep(savedStep);
                    onNavigate({ kind: 'trigger', triggerId: value.id });
                  }
                }}
                onPublished={(value) => {
                  updateDocument(value);
                  onNavigate({ kind: 'trigger_embed', triggerId: value.id });
                }}
              />
            )}
            <Presence>
              {loading ? null : failure ? (
                <div role="alert">
                  <p>This Trigger could not be loaded.</p>
                  <Button
                    icon={<IconRefreshOutline16 />}
                    onClick={() => setReload((value) => value + 1)}
                  >
                    Try again
                  </Button>
                </div>
              ) : null}
            </Presence>
          </>
        </div>
      </div>
    </section>
  );
}
