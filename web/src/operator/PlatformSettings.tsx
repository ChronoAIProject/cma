import { Presence } from '../ui/Presence';
import { useEffect, useId, useRef, useState } from 'react';
import type { PlatformSettings as Settings, StoredPlatformSettings } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { IconCheckOutline16, IconRefreshOutline16 } from '../ui/icons';
import type { OperatorClient } from './client';
import { operatorFailure, type OperatorFailure } from './failure';
import { FailureNotice } from './records';
import { reviewSettingsDraft } from './settingsDraft';

export function PlatformSettings({ client }: { readonly client: OperatorClient }) {
  const [state, setState] = useState<{ stored?: StoredPlatformSettings; error?: OperatorFailure }>(
    {},
  );
  const [reload, setReload] = useState(0);
  useEffect(() => {
    const controller = new AbortController();
    client.admin.readSettings({ signal: controller.signal }).then(
      (stored) => {
        if (!controller.signal.aborted) setState({ stored });
      },
      (error: unknown) => {
        if (!controller.signal.aborted) setState({ error: operatorFailure(error) });
      },
    );
    return () => controller.abort();
  }, [client, reload]);
  return (
    <section className="operator-settings" aria-label="Platform settings">
      <h2>Platform settings</h2>
      <FailureNotice failure={state.error} />
      {state.stored === undefined ? (
        <>
          {state.error === undefined ? (
            <div className="operator-skeleton" role="status" aria-label="Loading settings">
              <span />
              <span />
              <span />
            </div>
          ) : (
            <Button
              icon={<IconRefreshOutline16 />}
              onClick={() => {
                setState({});
                setReload((value) => value + 1);
              }}
            >
              Retry settings
            </Button>
          )}
        </>
      ) : (
        <SettingsEditor client={client} initial={state.stored} />
      )}
    </section>
  );
}

function SettingsEditor({
  client,
  initial,
}: {
  readonly client: OperatorClient;
  readonly initial: StoredPlatformSettings;
}) {
  const [draft, setDraft] = useState<Settings>(initial.settings);
  const [base, setBase] = useState<Settings>(initial.settings);
  const [revision, setRevision] = useState(initial.revision);
  const [current, setCurrent] = useState<StoredPlatformSettings>();
  const [failure, setFailure] = useState<OperatorFailure>();
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [needsReview, setNeedsReview] = useState(false);
  const [previewCount, setPreviewCount] = useState<number>();
  const [pauseNowResult, setPauseNowResult] = useState<string>();
  const [pauseNowBusy, setPauseNowBusy] = useState(false);
  const submitting = useRef(false);
  const update = (value: Settings) => {
    setDraft(value);
    setSaved(false);
  };
  const reread = async () => {
    try {
      setCurrent(await client.admin.readSettings());
    } catch (error) {
      setFailure(operatorFailure(error));
    }
  };
  const save = async () => {
    if (submitting.current || needsReview) return;
    submitting.current = true;
    setBusy(true);
    setFailure(undefined);
    setSaved(false);
    try {
      const stored = await client.admin.replaceSettings({
        expected_revision: revision,
        settings: draft,
      });
      setDraft(stored.settings);
      setBase(stored.settings);
      setRevision(stored.revision);
      setCurrent(undefined);
      setSaved(true);
    } catch (error) {
      const problem = operatorFailure(error);
      setFailure(problem);
      if (problem.conflict || problem.uncertain) {
        setNeedsReview(true);
        await reread();
      }
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };
  const disabled = busy;
  const fieldError = (path: string) =>
    failure?.field?.replace(/^settings\./, '') === path ? failure.message : undefined;
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <p className="operator-notice">Revision {revision}</p>
      <FailureNotice failure={failure} />
      <Presence>
        {needsReview ? (
          <div className="operator-conflict" role="status">
            <p>
              Your draft is retained.{' '}
              {current === undefined
                ? 'The current settings could not be read.'
                : `The current revision is ${current.revision}.`}
            </p>
            <div className="operator-actions">
              <Button
                disabled={busy || current === undefined}
                onClick={() => {
                  if (current !== undefined) {
                    setDraft(current.settings);
                    setBase(current.settings);
                    setRevision(current.revision);
                    setCurrent(undefined);
                    setNeedsReview(false);
                    setFailure(undefined);
                  }
                }}
              >
                Load current values
              </Button>
              <Button
                disabled={busy || current === undefined}
                onClick={() => {
                  if (current !== undefined) {
                    setDraft(reviewSettingsDraft(base, draft, current.settings));
                    setBase(current.settings);
                    setRevision(current.revision);
                    setCurrent(undefined);
                    setNeedsReview(false);
                    setFailure(undefined);
                  }
                }}
              >
                Review retained draft
              </Button>
              {current === undefined ? (
                <Button
                  disabled={busy}
                  icon={<IconRefreshOutline16 />}
                  onClick={() => void reread()}
                >
                  Read current settings
                </Button>
              ) : null}
            </div>
          </div>
        ) : null}
      </Presence>
      <section>
        <h3>Inactivity</h3>
        <label className="operator-switch">
          <input
            type="checkbox"
            disabled={disabled}
            checked={draft.inactivity.auto_pause_enabled ?? false}
            onChange={(event) =>
              update({
                ...draft,
                inactivity: {
                  ...draft.inactivity,
                  auto_pause_enabled: event.target.checked,
                },
              })
            }
          />
          Enable auto pause
        </label>
        {(draft.inactivity.auto_pause_enabled ?? false) ? (
          <IdleThresholdFields
            seconds={draft.inactivity.pause_after_seconds}
            disabled={disabled}
            onChange={(seconds) =>
              update({
                ...draft,
                inactivity: { ...draft.inactivity, pause_after_seconds: seconds },
              })
            }
          />
        ) : null}
        <NumberField
          label="Pause after (seconds)"
          value={draft.inactivity.pause_after_seconds}
          minimum={60}
          maximum={86400}
          disabled={disabled}
          error={fieldError('inactivity.pause_after_seconds')}
          onChange={(value) =>
            update({ ...draft, inactivity: { ...draft.inactivity, pause_after_seconds: value } })
          }
        />
        <div className="operator-actions">
          <Button
            disabled={disabled || pauseNowBusy}
            onClick={() => {
              void client.admin
                .previewAutoPause({})
                .then((result) => setPreviewCount(result.candidates));
            }}
          >
            Preview idle Sandboxes
          </Button>
          <Button
            disabled={disabled || pauseNowBusy}
            onClick={() => {
              if (
                !window.confirm(
                  'Pause every currently idle Sandbox now? Active work and Always on Sandboxes are rechecked.',
                )
              )
                return;
              setPauseNowBusy(true);
              setPauseNowResult(undefined);
              void client.admin
                .pauseIdleNow(true, {})
                .then((result) =>
                  setPauseNowResult(`${result.scheduled} Sandboxes scheduled to pause.`),
                )
                .finally(() => setPauseNowBusy(false));
            }}
          >
            {pauseNowBusy ? 'Pausing idle Sandboxes' : 'Pause idle now'}
          </Button>
          {previewCount === undefined ? null : (
            <span role="status">{previewCount} Sandboxes are candidates.</span>
          )}
          {pauseNowResult === undefined ? null : <span role="status">{pauseNowResult}</span>}
        </div>
      </section>
      <section>
        <h3>Retention</h3>
        <div className="operator-settings-grid">
          <NumberField
            label="Agent log retention (days)"
            value={draft.retention.agent_log_days}
            error={fieldError('retention.agent_log_days')}
            disabled={disabled}
            onChange={(value) =>
              update({ ...draft, retention: { ...draft.retention, agent_log_days: value } })
            }
          />
        </div>
      </section>
      <section>
        <h3>Limits</h3>
        <div className="operator-settings-grid">
          <NumberField
            label="Sandboxes per user"
            value={draft.limits.max_sandboxes_per_user}
            error={fieldError('limits.max_sandboxes_per_user')}
            disabled={disabled}
            onChange={(value) =>
              update({ ...draft, limits: { ...draft.limits, max_sandboxes_per_user: value } })
            }
          />
          <NumberField
            label="Visitor Sandboxes"
            value={draft.limits.visitor_max_sandboxes}
            error={fieldError('limits.visitor_max_sandboxes')}
            disabled={disabled}
            onChange={(value) =>
              update({ ...draft, limits: { ...draft.limits, visitor_max_sandboxes: value } })
            }
          />
          <NumberField
            label="Visitor launches per day"
            value={draft.limits.visitor_max_launches_per_day}
            error={fieldError('limits.visitor_max_launches_per_day')}
            disabled={disabled}
            onChange={(value) =>
              update({ ...draft, limits: { ...draft.limits, visitor_max_launches_per_day: value } })
            }
          />
        </div>
      </section>
      <section>
        <h3>Triggers</h3>
        <label className="operator-switch">
          <input
            type="checkbox"
            disabled={disabled}
            checked={draft.triggers.enabled}
            onChange={(event) =>
              update({ ...draft, triggers: { ...draft.triggers, enabled: event.target.checked } })
            }
          />
          Triggers enabled
        </label>
        <label className="operator-switch">
          <input
            type="checkbox"
            disabled={disabled}
            checked={draft.triggers.orchestration_paused}
            onChange={(event) =>
              update({
                ...draft,
                triggers: { ...draft.triggers, orchestration_paused: event.target.checked },
              })
            }
          />
          Orchestration paused
        </label>
      </section>
      <div className="operator-actions">
        <Button
          type="submit"
          variant="primary"
          disabled={busy || needsReview}
          icon={<IconCheckOutline16 />}
        >
          {busy ? 'Saving' : 'Save settings'}
        </Button>
        <Presence>
          {saved ? <p role="status">Settings saved at revision {revision}.</p> : null}
        </Presence>
      </div>
    </form>
  );
}

function IdleThresholdFields({
  seconds,
  disabled,
  onChange,
}: {
  readonly seconds: number;
  readonly disabled: boolean;
  readonly onChange: (seconds: number) => void;
}) {
  const safeSeconds = Number.isFinite(seconds) ? Math.max(0, seconds) : 0;
  const hours = Math.floor(safeSeconds / 3600);
  const minutes = Math.floor((safeSeconds % 3600) / 60);
  const remainder = safeSeconds % 60;
  const change = (part: 'hours' | 'minutes' | 'seconds', value: number) => {
    const next =
      part === 'hours'
        ? value * 3600 + minutes * 60 + remainder
        : part === 'minutes'
          ? hours * 3600 + value * 60 + remainder
          : hours * 3600 + minutes * 60 + value;
    onChange(next);
  };
  return (
    <div className="operator-settings-grid" aria-label="Automatic pause idle threshold">
      <NumberField
        label="Pause after hours"
        value={hours}
        minimum={0}
        disabled={disabled}
        onChange={(value) => change('hours', value)}
      />
      <NumberField
        label="Pause after minutes"
        value={minutes}
        minimum={0}
        maximum={59}
        disabled={disabled}
        onChange={(value) => change('minutes', value)}
      />
      <NumberField
        label="Pause after seconds"
        value={remainder}
        minimum={0}
        maximum={59}
        disabled={disabled}
        onChange={(value) => change('seconds', value)}
      />
      <p className="operator-notice">Choose at least 60 seconds and at most 24 hours.</p>
    </div>
  );
}

function NumberField({
  label,
  value,
  minimum = 0,
  maximum = 4_294_967_295,
  step = 1,
  disabled,
  error,
  onChange,
}: {
  readonly label: string;
  readonly value: number;
  readonly minimum?: number;
  readonly maximum?: number;
  readonly step?: number | 'any';
  readonly disabled: boolean;
  readonly error?: string;
  readonly onChange: (value: number) => void;
}) {
  const id = useId();
  return (
    <label className="operator-field">
      <span>{label}</span>
      <Input
        aria-label={label}
        type="number"
        value={Number.isNaN(value) ? '' : value}
        min={minimum}
        max={maximum}
        step={step}
        required
        disabled={disabled}
        aria-invalid={error !== undefined}
        aria-describedby={error === undefined ? undefined : id}
        onChange={(event) =>
          onChange(event.target.value === '' ? Number.NaN : Number(event.target.value))
        }
      />
      {error === undefined ? null : (
        <span id={id} className="operator-failure">
          {error}
        </span>
      )}
    </label>
  );
}
