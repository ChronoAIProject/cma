import { observeRead } from './transport/readObserver';
import { retrySchedule } from './transport/readRetry';
import { useEffect, useRef, useState } from 'react';
import { useCmaClient, useProfileViewsClient } from '../api/context';
import type { AgentSkillsView } from '../api/client';
import { AgentSkillsError } from '../api/rest/agentSkills';
import { catalogFailure } from '../profiles/catalogFailure';
import { useSkillCatalog } from './useSkillCatalog';
import { isSkillVersion } from '../profiles/validation';
import { Button } from '../ui/Button';
import { IconChevronRightOutline14, IconWarningOutline16 } from '../ui/icons';
import { Input } from '../ui/Input';
import { Picker } from '../ui/Picker';
import './SkillsSettings.css';

interface SkillsSettingsProps {
  readonly catalogOpen?: boolean;
  readonly onClose?: () => void;
  readonly agentId: string;
  readonly viewerId?: string;
  readonly active: boolean;
  readonly busy: boolean;
}

export function SkillsSettings(props: SkillsSettingsProps) {
  return <SkillsSettingsForm key={JSON.stringify([props.viewerId, props.agentId])} {...props} />;
}

function SkillsSettingsForm({
  agentId,
  active,
  busy,
  catalogOpen = false,
  onClose,
}: SkillsSettingsProps) {
  const client = useCmaClient();
  const catalog = useProfileViewsClient()?.authoring;
  const [saved, setSaved] = useState<AgentSkillsView>();
  const [statusError, setStatusError] = useState<string>();
  const [reload, setReload] = useState(0);
  const catalogState = useSkillCatalog(catalog, active);
  const { entries, query, next } = catalogState;
  const catalogLoading = catalogState.phase === 'loading';
  const catalogError =
    catalogState.phase === 'failed' ? catalogFailure(catalogState.failure) : undefined;
  const [versions, setVersions] = useState<Record<string, string>>({});
  const [sending, setSending] = useState(false);
  const [installation, setInstallation] = useState<{
    intent?: { id: string; version: string; key: string };
    error?: string;
  }>({});
  const { intent, error: installError } = installation;
  const mutation = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => mutation.current?.abort(), []);
  useEffect(() => {
    if (!active || !client.readAgentSkills) return;
    let failures = 0;
    const observer = observeRead(async (signal) => {
      try {
        const value = await client.readAgentSkills!(agentId, { signal: signal });
        if (signal.aborted) return;
        setSaved(value);
        setStatusError(undefined);
        setInstallation((current) =>
          current.intent &&
          value.items.some(
            (item) =>
              item.id === current.intent?.id &&
              item.version === current.intent.version &&
              item.status !== 'failed',
          )
            ? {}
            : current,
        );
        failures = 0;
        if (value.pending) return { next: 1000, cooldown: 1000 };
      } catch (cause) {
        if (!signal.aborted)
          setStatusError(
            cause instanceof Error
              ? cause.message
              : 'Skill installation status could not be loaded.',
          );
        if (!signal.aborted) return retrySchedule(cause, ++failures);
      }
    });
    return () => observer.dispose();
  }, [active, agentId, busy, client, reload]);
  const install = async (id: string, version: string) => {
    if (
      !active ||
      mutation.current ||
      busy ||
      saved?.pending ||
      !saved?.can_install ||
      !client.installAgentSkill
    )
      return;
    if (
      !intent &&
      (!catalogState.selectable ||
        !isSkillVersion(version) ||
        !entries.some((entry) => entry.id === id && entry.versions.includes(version)))
    )
      return;
    const retained = intent ?? { id, version, key: crypto.randomUUID() };
    const request = new AbortController();
    mutation.current = request;
    setInstallation({ intent: retained });
    setSending(true);
    try {
      const result = await client.installAgentSkill(
        agentId,
        { id: retained.id, version: retained.version },
        { idempotencyKey: retained.key, signal: request.signal },
      );
      if (request.signal.aborted) return;
      setSaved(result);
      setInstallation({});
      setReload((value) => value + 1);
    } catch (cause) {
      if (!request.signal.aborted) {
        setInstallation((current) => ({
          intent:
            cause instanceof AgentSkillsError &&
            [400, 401, 403, 404, 409, 422].includes(cause.status)
              ? undefined
              : current.intent,
          error: current.intent
            ? cause instanceof Error
              ? cause.message
              : 'Installation could not be confirmed. Retry the same request.'
            : undefined,
        }));
      }
    } finally {
      if (!request.signal.aborted) setSending(false);
      mutation.current = undefined;
    }
  };
  const disabled =
    !active ||
    !catalogState.selectable ||
    statusError !== undefined ||
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
      ) : statusError ? null : client.readAgentSkills ? (
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
      {statusError ? (
        <div className="agent-skills__failure" role="alert">
          <p>{statusError}</p>
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
      {installError ? <p role="alert">{installError}</p> : null}
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
      <details className="agent-skills__add" open={catalogOpen || undefined}>
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
            disabled={!catalogState.searchEnabled}
            onChange={(event) => catalogState.search(event.target.value)}
          />
          {!catalog ? (
            <p>The Ornn catalog is unavailable from this connection.</p>
          ) : catalogLoading ? (
            <p role="status">Loading available skills…</p>
          ) : catalogError ? (
            <div className="agent-skills__catalog-failure" role="alert">
              <span aria-hidden="true">
                <IconWarningOutline16 size={24} />
              </span>
              <h3>Couldn’t load the skills catalog</h3>
              <p>{catalogError.message}</p>
              <div className="agent-skills__catalog-diagnostics">
                {catalogError.status === undefined ? null : (
                  <p>HTTP status: {catalogError.status}</p>
                )}
                {catalogError.code === undefined ? null : <p>Code: {catalogError.code}</p>}
                {catalogError.requestId === undefined ? null : (
                  <p>Request ID: {catalogError.requestId}</p>
                )}
              </div>
              {catalogState.waiting ? (
                <p role="status">Waiting before another catalog request is allowed.</p>
              ) : null}
              {catalogError.retry ? (
                <Button
                  variant="primary"
                  disabled={catalogState.waiting}
                  onClick={catalogState.retry}
                >
                  {catalogState.cursor === undefined ? 'Retry Ornn catalog' : 'Retry skill page'}
                </Button>
              ) : null}
              {onClose ? <Button onClick={onClose}>Cancel</Button> : null}
            </div>
          ) : entries.length === 0 ? (
            <p>No matching Ornn skills.</p>
          ) : null}
          <ul
            className="agent-skills__entries"
            aria-label="Available Ornn skills"
            aria-busy={catalogLoading && catalog !== undefined}
          >
            {entries.map((entry) => {
              const available = entry.versions.filter(isSkillVersion);
              const existing = saved?.items.find((item) => item.id === entry.id);
              const selected = versions[entry.id];
              const version =
                existing?.version ??
                (selected && available.includes(selected) ? selected : (available[0] ?? ''));
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
          {next ? (
            <button
              type="button"
              className="agent-details__link"
              disabled={catalogState.phase !== 'ready'}
              onClick={catalogState.more}
            >
              More Ornn skills
            </button>
          ) : null}
        </div>
      </details>
    </section>
  );
}
