import { Presence } from '../ui/Presence';
import { useEffect, useRef, useState } from 'react';
import type { PlatformSettings as Settings, StoredPlatformSettings } from '@cma/client';
import { Button } from '../ui/Button';
import { IconCheckOutline16, IconRefreshOutline16 } from '../ui/icons';
import type { OperatorClient } from './client';
import { operatorFailure, type OperatorFailure } from './failure';
import { FailureNotice } from './records';
import { usageSettingsChanged } from '../usage/metrics/useMeasuredUsage';
import { reviewSettingsDraft } from './settingsDraft';
import { spendEditors } from './SpendControls';
import { SpendStatus } from './SpendStatus';
import { budgetChanged, spendBudget, spendListConflict } from './spendBudget';
import { validateSpendBudget, type SpendErrors } from './spendValidation';
import { SettingCard, SettingsOverview } from './SettingCard';
import { settingsInventory } from './settingsInventory';
import { settingEditors } from './settingsEditors';
import { AutoPauseControls } from './AutoPauseControls';
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
  const [changes, setChanges] = useState(initial.setting_changes);
  const [availability, setAvailability] = useState<SpendAvailability>(initial);
  const [current, setCurrent] = useState<StoredPlatformSettings>();
  const [failure, setFailure] = useState<OperatorFailure>();
  const [busy, setBusy] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [saved, setSaved] = useState(false);
  const [needsReview, setNeedsReview] = useState<'draft' | 'write'>();
  const [spendErrors, setSpendErrors] = useState<SpendErrors>({});
  const submitting = useRef(false);
  const spendChanged = budgetChanged(base.model_budget, draft.model_budget);
  // PUT restores an omitted policy before the backend evaluates the write gates.
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
  const refresh = async () => {
    if (submitting.current) return;
    // Uncertain writes still require review even when the visible draft is clean.
    const retainDraft = needsReview === 'write' || JSON.stringify(draft) !== JSON.stringify(base);
    submitting.current = true;
    setRefreshing(true);
    setBusy(true);
    setFailure(undefined);
    try {
      const latest = await client.admin.readSettings();
      setAvailability(latest);
      if (retainDraft) {
        setCurrent(latest);
        setNeedsReview(needsReview === 'write' ? 'write' : 'draft');
      } else {
        setDraft(latest.settings);
        setBase(latest.settings);
        setRevision(latest.revision);
        setChanges(latest.setting_changes);
        setCurrent(undefined);
        setNeedsReview(undefined);
        setSpendErrors({});
        setSaved(false);
      }
    } catch (error) {
      setFailure(operatorFailure(error));
    } finally {
      submitting.current = false;
      setRefreshing(false);
      setBusy(false);
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
      setChanges(stored.setting_changes);
      setAvailability(stored);
      setCurrent(undefined);
      setSaved(true);
      usageSettingsChanged();
    } catch (error) {
      const problem = operatorFailure(error);
      setFailure(problem);
      if (problem.code === 'model_budget_writes_unavailable') setAvailability({});
      if (problem.conflict || problem.uncertain) {
        setNeedsReview('write');
        await reread();
      }
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };
  const disabled = busy;
  const listConflict =
    !!current &&
    spendListConflict(base.model_budget, draft.model_budget, current.settings.model_budget);
  const canReviewBudget = !current || !spendChanged || !!spendBudget(current.settings.model_budget);
  const fieldError = (path: string) =>
    failure?.field?.replace(/^settings\./, '') === path ? failure.message : undefined;
  const displayed = current ?? { revision, settings: base, setting_changes: changes };
  const inventory = settingsInventory(displayed.settings);
  const editors = settingEditors(draft, disabled, fieldError, update);
  const spend = spendEditors({
    value: draft.model_budget,
    disabled: disabled || !writesAvailable,
    writesAvailable,
    requestWritesAvailable: availability.model_request_budget_writes_available === true,
    platformWritesAvailable: availability.platform_spend_writes_available === true,
    errors: spendErrors,
    onChange: (model_budget) => {
      update({ ...draft, model_budget });
      setSpendErrors({});
    },
  });
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <p className="operator-notice">Revision {displayed.revision}</p>
      <SettingsOverview inventory={inventory} />
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
                    setChanges(current.setting_changes);
                    setAvailability(current);
                    setCurrent(undefined);
                    setNeedsReview(undefined);
                    setFailure(undefined);
                    setSpendErrors({});
                  }
                }}
              >
                Load current values
              </Button>
              <Button
                disabled={busy || current === undefined || !canReviewBudget || listConflict}
                onClick={() => {
                  if (current !== undefined) {
                    setDraft(reviewSettingsDraft(base, draft, current.settings));
                    setBase(current.settings);
                    setRevision(current.revision);
                    setChanges(current.setting_changes);
                    setAvailability(current);
                    setCurrent(undefined);
                    setNeedsReview(undefined);
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
            {listConflict ? (
              <p>
                Owner overrides or request models changed both here and on the server. Your draft
                remains visible. Load current values and reapply those list edits before saving.
              </p>
            ) : null}
            {!canReviewBudget ? (
              <p>
                The current server no longer exposes the spend-controls contract. Your draft is
                retained; load current values before saving other settings.
              </p>
            ) : null}
          </div>
        ) : null}
      </Presence>
      <div className="operator-setting-cards">
        {inventory.map((setting) => {
          const spendEditor = spend[setting.id];
          return (
            <SettingCard
              key={setting.id}
              setting={{ ...setting, readOnly: setting.readOnly ?? spendEditor?.readOnly }}
              change={displayed.setting_changes?.[setting.id]}
              canReadAudit={canReadAudit}
              disabled={disabled}
            >
              {spendEditor?.editor ?? editors[setting.id]}
            </SettingCard>
          );
        })}
      </div>
      {!writesAvailable ||
      availability.model_request_budget_writes_available !== true ||
      availability.platform_spend_writes_available !== true ? (
        <div className="operator-actions">
          <Button disabled={busy} onClick={() => void refresh()}>
            Refresh settings availability
          </Button>
          {spendSaveBlocked ? (
            <p role="status">
              Saving settings is not available yet for this spend policy. Your changes are retained.
            </p>
          ) : null}
        </div>
      ) : null}
      <SpendStatus client={client} disabled={disabled} key={`spend-${revision}`} />
      <section className="operator-settings-tools" aria-label="Idle Sandbox operations">
        <h3>Idle Sandbox operations</h3>
        <AutoPauseControls client={client} disabled={disabled} key={revision} />
      </section>
      <div className="operator-actions">
        <Button
          type="submit"
          variant="primary"
          disabled={busy || needsReview !== undefined || spendSaveBlocked}
          icon={<IconCheckOutline16 />}
        >
          {refreshing ? 'Refreshing' : busy ? 'Saving' : 'Save settings'}
        </Button>
        <Presence>
          {saved ? <p role="status">Settings saved at revision {revision}.</p> : null}
        </Presence>
      </div>
    </form>
  );
}
