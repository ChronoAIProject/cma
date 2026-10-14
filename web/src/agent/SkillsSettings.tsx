import { useEffect, useRef, useState } from 'react';
import { useCmaClient, useProfileViewsClient } from '../api/context';
import type { AgentSkillsView } from '../api/client';
import { AgentSkillsError } from '../api/rest/agentSkills';
import type { SkillCatalogEntry } from '../profiles/client';
import { isSkillVersion } from '../profiles/validation';
import { Button } from '../ui/Button';
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
  return (
    <section className="agent-skills" aria-label="Agent skills">
      <h2>Skills</h2>
      {saved ? (
        saved.items.length ? (
          <ul className="agent-skills__list" aria-label="Agent skill status" aria-live="polite">
            {saved.items.map((item) => (
              <li key={item.id}>
                <strong>
                  {item.name ?? entries.find((entry) => entry.id === item.id)?.name ?? item.id}
                </strong>
                <span>
                  {item.version} ·{' '}
                  {item.status === 'installed'
                    ? 'Installed'
                    : item.status === 'pending'
                      ? 'Installing…'
                      : 'Installation failed'}
                </span>
                {item.error ? <p role="alert">{item.error}</p> : null}
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
        <div role="alert">
          <p>{error}</p>
          <Button disabled={sending} onClick={() => setReload((value) => value + 1)}>
            Refresh skill status
          </Button>
        </div>
      ) : null}
      {intent && !sending ? (
        <Button
          disabled={busy || saved?.pending || !saved?.can_install}
          onClick={() => void install(intent.id, intent.version)}
        >
          Retry installation
        </Button>
      ) : null}
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
        className="agent-skills__list"
        aria-label="Available Ornn skills"
        aria-busy={catalogLoading && catalog !== undefined}
      >
        {entries.map((entry) => {
          const available = entry.versions.filter(isSkillVersion);
          const version = versions[entry.id] ?? available[0] ?? '';
          const existing = saved?.items.find((item) => item.id === entry.id);
          return (
            <li key={entry.id}>
              <strong>{entry.name}</strong>
              {entry.description ? <p>{entry.description}</p> : null}
              <div className="agent-skills__actions">
                <Picker
                  label={`Version for ${entry.name}`}
                  value={version}
                  options={available.map((value) => ({ value, label: value }))}
                  disabled={
                    disabled ||
                    available.length === 0 ||
                    existing?.status === 'installed' ||
                    existing?.status === 'pending'
                  }
                  onChange={(value) =>
                    setVersions((current) => ({ ...current, [entry.id]: value }))
                  }
                />
                <Button
                  disabled={
                    disabled ||
                    !version ||
                    existing?.status === 'installed' ||
                    existing?.status === 'pending'
                  }
                  aria-label={`Install ${entry.name}`}
                  onClick={() => void install(entry.id, version)}
                >
                  {existing?.status === 'installed'
                    ? 'Installed'
                    : existing?.status === 'pending'
                      ? 'Installing…'
                      : 'Install'}
                </Button>
              </div>
              {available.length === 0 ? <p>No published release is available.</p> : null}
            </li>
          );
        })}
      </ul>
      {!catalog ? (
        <p>The Ornn catalog is unavailable from this connection.</p>
      ) : catalogLoading ? (
        <p role="status">Loading available skills…</p>
      ) : catalogError ? (
        <div role="alert">
          <p>{catalogError}</p>
          <Button
            onClick={() => {
              setCatalogLoading(true);
              setCatalogReload((value) => value + 1);
            }}
          >
            Retry Ornn catalog
          </Button>
        </div>
      ) : entries.length === 0 ? (
        <p>No matching Ornn skills.</p>
      ) : null}
      {next ? (
        <Button
          disabled={catalogLoading}
          onClick={() => {
            setCursor(next);
            setCatalogLoading(true);
          }}
        >
          More Ornn skills
        </Button>
      ) : null}
    </section>
  );
}
