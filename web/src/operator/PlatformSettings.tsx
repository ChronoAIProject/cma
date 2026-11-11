import { Presence } from '../ui/Presence';
import { useEffect, useRef, useState } from 'react';
import type { PlatformSettings as Settings, StoredPlatformSettings } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { IconCheckOutline16, IconRefreshOutline16 } from '../ui/icons';
import type { OperatorClient } from './client';
import { operatorFailure, type OperatorFailure } from './failure';
import { FailureNotice } from './records';
import { usageSettingsChanged } from '../usage/metrics/useMeasuredUsage';
import { reviewSettingsDraft } from './settingsDraft';
import { IdleThresholdFields, NumberField } from './SettingsFields';
import { SpendControls } from './SpendControls';
import { budgetChanged, spendBudget } from './spendBudget';
import { validateSpendBudget, type SpendErrors } from './spendValidation';
import { SettingsAudit } from './SettingsAudit';
import { spendAvailability, type SpendAvailability } from './spendAvailability';

export function PlatformSettings({
  client,
  canReadAudit = false,
}: {
  readonly client: OperatorClient;
  readonly canReadAudit?: boolean;
}) {
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
        <SettingsEditor client={client} initial={state.stored} canReadAudit={canReadAudit} />
      )}
    </section>
  );
}

function SettingsEditor({
  client,
  initial,
  canReadAudit,
}: {
  readonly client: OperatorClient;
  readonly initial: StoredPlatformSettings;
  readonly canReadAudit: boolean;
}) {
  const [draft, setDraft] = useState<Settings>(initial.settings);
  const [base, setBase] = useState<Settings>(initial.settings);
  const [revision, setRevision] = useState(initial.revision);
  const [availability, setAvailability] = useState<SpendAvailability>(initial);
  const [current, setCurrent] = useState<StoredPlatformSettings>();
  const [failure, setFailure] = useState<OperatorFailure>();
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [needsReview, setNeedsReview] = useState(false);
  const [previewCount, setPreviewCount] = useState<number>();
  const [pauseNowResult, setPauseNowResult] = useState<string>();
  const [pauseNowBusy, setPauseNowBusy] = useState(false);
  const [spendErrors, setSpendErrors] = useState<SpendErrors>({});
  const submitting = useRef(false);
  const spendChanged = budgetChanged(base.model_budget, draft.model_budget);
  // PUT restores an omitted policy before the backend evaluates either gate.
  const effectiveBudget = draft.model_budget === undefined ? base.model_budget : draft.model_budget;
  const { edit: writesAvailable, save: canSave } = spendAvailability(availability, effectiveBudget);
  const spendSaveBlocked = !canSave;
  const update = (value: Settings) => {
    setDraft(value);
    setSaved(false);
  };
  const reread = async () => {
    try {
      const latest = await client.admin.readSettings();
      setCurrent(latest);
      setAvailability(latest);
    } catch (error) {
      setFailure(operatorFailure(error));
    }
  };
  const save = async () => {
    if (submitting.current || needsReview || spendSaveBlocked) return;
    const budget = spendBudget(draft.model_budget);
    const errors = budget && writesAvailable ? validateSpendBudget(budget) : {};
    setSpendErrors(errors);
    if (Object.keys(errors).length) return;
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
      setAvailability(stored);
      setCurrent(undefined);
      setSaved(true);
      usageSettingsChanged();
    } catch (error) {
      const problem = operatorFailure(error);
      setFailure(problem);
      if (problem.code === 'model_budget_writes_unavailable') setAvailability({});
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
  const canReviewBudget = !current || !spendChanged || !!spendBudget(current.settings.model_budget);
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
      <SettingsAudit key={revision} client={client} revision={revision} allowed={canReadAudit} />
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
                    setAvailability(current);
                    setCurrent(undefined);
                    setNeedsReview(false);
                    setFailure(undefined);
                    setSpendErrors({});
                  }
                }}
              >
                Load current values
              </Button>
              <Button
                disabled={busy || current === undefined || !canReviewBudget}
                onClick={() => {
                  if (current !== undefined) {
                    setDraft(reviewSettingsDraft(base, draft, current.settings));
                    setBase(current.settings);
                    setRevision(current.revision);
                    setAvailability(current);
                    setCurrent(undefined);
                    setNeedsReview(false);
                    setFailure(undefined);
                    setSpendErrors({});
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
            {!canReviewBudget ? (
              <p>
                The current server no longer exposes the spend-controls contract. Your draft is
                retained; load current values before saving other settings.
              </p>
            ) : null}
          </div>
        ) : null}
      </Presence>
      <SpendControls
        value={draft.model_budget}
        disabled={disabled || !writesAvailable}
        writesAvailable={writesAvailable}
        errors={spendErrors}
        onChange={(model_budget) => {
          update({ ...draft, model_budget });
          setSpendErrors({});
        }}
      />
      {!writesAvailable ? (
        <div className="operator-actions">
          <Button
            disabled={busy}
            onClick={() => {
              setNeedsReview(true);
              void reread();
            }}
          >
            Refresh settings availability
          </Button>
          {spendSaveBlocked ? (
            <p role="status">
              Saving settings is not available yet for this spend policy. Your changes are retained.
            </p>
          ) : null}
        </div>
      ) : null}
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
        ) : (
          <NumberField
            label="Pause after (seconds)"
            value={draft.inactivity.pause_after_seconds}
            minimum={60}
            maximum={86400}
            disabled={disabled}
            error={fieldError('inactivity.pause_after_seconds')}
            onChange={(value) =>
              update({
                ...draft,
                inactivity: { ...draft.inactivity, pause_after_seconds: value },
              })
            }
          />
        )}
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
        <h3>Usage</h3>
        <label className="operator-switch">
          <input
            type="checkbox"
            disabled={disabled}
            checked={draft.usage_reporting?.measured_visible === true}
            onChange={(event) =>
              update({
                ...draft,
                usage_reporting: {
                  ...draft.usage_reporting,
                  measured_visible: event.target.checked,
                },
              })
            }
          />
          Show measured usage
        </label>
        <p className="operator-notice">
          Show measured CPU, memory and disk alongside allocated resources when measurements are
          available. Allocated resources remain visible when this setting is off.
        </p>
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
          disabled={busy || needsReview || spendSaveBlocked}
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
