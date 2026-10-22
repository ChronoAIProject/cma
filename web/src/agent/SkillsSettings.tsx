import { useEffect, useRef, useState } from 'react';
import { useCmaClient, useProfileViewsClient } from '../api/context';
import type { AgentSkillsView } from '../api/client';
import { AgentSkillsError } from '../api/rest/agentSkills';
import type { SkillCatalogEntry } from '../profiles/client';
import { isSkillVersion } from '../profiles/validation';
import { Button } from '../ui/Button';
import { IconChevronRightOutline14 } from '../ui/icons';
import { Input } from '../ui/Input';
import { Picker } from '../ui/Picker';
import './SkillsSettings.css';

export function SkillsSettings({
  agentId,
  active,
  busy,
}: {
  readonly agentId: string;
  readonly active: boolean;
  readonly busy: boolean;
}) {
  const client = useCmaClient();
  const catalog = useProfileViewsClient()?.authoring;
  const [saved, setSaved] = useState<AgentSkillsView>();
  const [error, setError] = useState<string>();
  const [reload, setReload] = useState(0);
  const [query, setQuery] = useState('');
  const [cursor, setCursor] = useState<string>();
  const [catalogReload, setCatalogReload] = useState(0);
  const [entries, setEntries] = useState<readonly SkillCatalogEntry[]>([]);
  const [next, setNext] = useState<string>();
  const [catalogLoading, setCatalogLoading] = useState(true);
  const [catalogError, setCatalogError] = useState<string>();
  const [versions, setVersions] = useState<Record<string, string>>({});
  const [sending, setSending] = useState(false);
  const [intent, setIntent] = useState<{ id: string; version: string; key: string }>();
  const mutation = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => mutation.current?.abort(), []);
  useEffect(() => {
    if (!active || !client.readAgentSkills) return;
    const request = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      try {
        const value = await client.readAgentSkills!(agentId, { signal: request.signal });
        if (request.signal.aborted) return;
        setSaved(value);
        setError(undefined);
        setIntent((current) =>
          current &&
          value.items.some(
            (item) =>
              item.id === current.id &&
              item.version === current.version &&
              item.status !== 'failed',
          )
            ? undefined
            : current,
        );
        if (value.pending) timer = setTimeout(() => void poll(), 1000);
      } catch (cause) {
        if (!request.signal.aborted)
          setError(
            cause instanceof Error
              ? cause.message
              : 'Skill installation status could not be loaded.',
          );
      }
    };
    void poll();
    return () => {
      request.abort();
      clearTimeout(timer);
    };
  }, [active, agentId, busy, client, reload]);
  useEffect(() => {
    if (!active || !catalog) return;
    const request = new AbortController();
    const timer = setTimeout(() => {
      void catalog
        .searchSkills(query, { cursor, signal: request.signal })
        .then((page) => {
          if (request.signal.aborted) return;
          if (page.next !== undefined && page.next === cursor)
            throw new Error('Skill catalog page did not advance.');
          setEntries((previous) =>
            cursor === undefined
              ? page.items
              : [
                  ...previous,
                  ...page.items.filter((item) => !previous.some((entry) => entry.id === item.id)),
                ],
          );
          setNext(page.next);
          setCatalogError(undefined);
        })
        .catch(() => {
          if (!request.signal.aborted) setCatalogError('Ornn skills could not be loaded.');
        })
        .finally(() => {
          if (!request.signal.aborted) setCatalogLoading(false);
        });
    }, 200);
    return () => {
      request.abort();
      clearTimeout(timer);
    };
  }, [active, catalog, query, cursor, catalogReload]);
  const install = async (id: string, version: string) => {
    if (
      mutation.current ||
      busy ||
      saved?.pending ||
      !saved?.can_install ||
      !client.installAgentSkill
    )
      return;
    const retained = intent ?? { id, version, key: crypto.randomUUID() };
    const request = new AbortController();
    mutation.current = request;
    setIntent(retained);
    setSending(true);
    setError(undefined);
    try {
      const result = await client.installAgentSkill(
        agentId,
        { id: retained.id, version: retained.version },
        { idempotencyKey: retained.key, signal: request.signal },
      );
      if (request.signal.aborted) return;
      setSaved(result);
      setIntent(undefined);
      setReload((value) => value + 1);
    } catch (cause) {
      if (!request.signal.aborted) {
        if (
          cause instanceof AgentSkillsError &&
          [400, 401, 403, 404, 409, 422].includes(cause.status)
        )
          setIntent(undefined);
        setError(
          cause instanceof Error
            ? cause.message
            : 'Installation could not be confirmed. Retry the same request.',
        );
      }
    } finally {
      if (!request.signal.aborted) setSending(false);
      mutation.current = undefined;
    }
  };
  const disabled =
    busy ||
    sending ||
    intent !== undefined ||
    !saved?.can_install ||
    saved.pending ||
    !client.installAgentSkill;
  const nameOf = (id: string, name?: string) =>
    name ?? entries.find((entry) => entry.id === id)?.name ?? id;
  return (
    <section className="agent-details__band agent-skills" aria-label="Agent skills">
      <h2>Skills</h2>
      {saved ? (
        saved.items.length ? (
          <ul
            className="agent-skills__installed"
            aria-label="Agent skill status"
            aria-live="polite"
          >
            {saved.items.map((item) => (
              <li key={item.id}>
                <strong title={nameOf(item.id, item.name)}>{nameOf(item.id, item.name)}</strong>
                <span title={`${item.version} · ${item.status}`}>
                  {item.version} ·{' '}
                  {item.status === 'installed'
                    ? 'Installed'
                    : item.status === 'pending'
                      ? 'Installing…'
                      : 'Installation failed'}
                </span>
                {item.error ? (
                  <p role="alert" title={item.error}>
                    {item.error}
                  </p>
                ) : null}
              </li>
            ))}
          </ul>
        ) : (
          <p>No additional skills installed.</p>
        )
      ) : client.readAgentSkills ? (
        <p role="status">Loading Agent skills…</p>
      ) : (
        <p>Skill installation is unavailable from this connection.</p>
      )}
      {saved?.pending ? <p role="status">Waiting for the Agent to confirm installation.</p> : null}
      {saved && !saved.can_install && !saved.pending && !busy ? (
        <p>
          {saved.unavailable_reason ?? 'This Agent runtime does not support installing skills yet.'}
        </p>
      ) : null}
      {busy ? (
        <p>Skills can be installed when the Agent is ready and its current turn has finished.</p>
      ) : null}
      {error ? (
        <div className="agent-skills__failure" role="alert">
          <p>{error}</p>
          <button
            type="button"
            className="agent-details__link"
            disabled={sending}
            onClick={() => setReload((value) => value + 1)}
          >
            Refresh skill status
          </button>
        </div>
      ) : null}
      {intent && !sending ? (
        <button
          type="button"
          className="agent-details__link"
          disabled={busy || saved?.pending || !saved?.can_install}
          onClick={() => void install(intent.id, intent.version)}
        >
          Retry installation
        </button>
      ) : null}
      {/*
       * The catalogue is management detail, so it waits behind a closed
       * disclosure; it still loads while the panel is open because installed
       * rows borrow its names.
       */}
      <details className="agent-skills__add">
        <summary>
          Add skills
          <span aria-hidden="true">
            <IconChevronRightOutline14 />
          </span>
        </summary>
        <div className="agent-skills__catalog">
          <label htmlFor={`agent-skill-search-${agentId}`}>Available Ornn skills</label>
          <Input
            id={`agent-skill-search-${agentId}`}
            aria-label="Search available Ornn skills"
            value={query}
            disabled={!catalog}
            onChange={(event) => {
              setQuery(event.target.value);
              setCursor(undefined);
              setEntries([]);
              setCatalogLoading(true);
            }}
          />
          <ul
            className="agent-skills__entries"
            aria-label="Available Ornn skills"
            aria-busy={catalogLoading && catalog !== undefined}
          >
            {entries.map((entry) => {
              const available = entry.versions.filter(isSkillVersion);
              const existing = saved?.items.find((item) => item.id === entry.id);
              const version = existing?.version ?? versions[entry.id] ?? available[0] ?? '';
              const description =
                existing?.status === 'failed'
                  ? 'This installation failed. This Agent cannot retry or replace this skill.'
                  : available.length === 0
                    ? 'No published release is available.'
                    : entry.description;
              return (
                <li key={entry.id}>
                  <strong title={entry.name}>{entry.name}</strong>
                  <p title={description}>{description ?? '\u00a0'}</p>
                  <div className="agent-skills__actions">
                    <Picker
                      label={`Version for ${entry.name}`}
                      size="sm"
                      value={version}
                      options={[
                        ...(existing && !available.includes(existing.version)
                          ? [
                              {
                                value: existing.version,
                                label: existing.version,
                                unavailable: 'Saved version is not in the available catalog.',
                              },
                            ]
                          : []),
                        ...available.map((value) => ({ value, label: value })),
                      ]}
                      disabled={disabled || available.length === 0 || existing !== undefined}
                      onChange={(value) =>
                        setVersions((current) => ({ ...current, [entry.id]: value }))
                      }
                    />
                    <Button
                      size="sm"
                      disabled={disabled || !version || existing !== undefined}
                      aria-label={`Install ${entry.name}`}
                      onClick={() => void install(entry.id, version)}
                    >
                      {existing?.status === 'installed'
                        ? 'Installed'
                        : existing?.status === 'pending'
                          ? 'Installing…'
                          : existing?.status === 'failed'
                            ? 'Installation failed'
                            : 'Install'}
                    </Button>
                  </div>
                </li>
              );
            })}
          </ul>
          {!catalog ? (
            <p>The Ornn catalog is unavailable from this connection.</p>
          ) : catalogLoading ? (
            <p role="status">Loading available skills…</p>
          ) : catalogError ? (
            <div className="agent-skills__failure" role="alert">
              <p>{catalogError}</p>
              <button
                type="button"
                className="agent-details__link"
                onClick={() => {
                  setCatalogLoading(true);
                  setCatalogReload((value) => value + 1);
                }}
              >
                Retry Ornn catalog
              </button>
            </div>
          ) : entries.length === 0 ? (
            <p>No matching Ornn skills.</p>
          ) : null}
          {next ? (
            <button
              type="button"
              className="agent-details__link"
              disabled={catalogLoading}
              onClick={() => {
                setCursor(next);
                setCatalogLoading(true);
              }}
            >
              More Ornn skills
            </button>
          ) : null}
        </div>
      </details>
    </section>
  );
}
