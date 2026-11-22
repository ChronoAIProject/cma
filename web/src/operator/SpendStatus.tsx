import { useRef, useState } from 'react';
import type { PlatformSpend, SpendClosure } from '@cma/client';
import { Button } from '../ui/Button';
import type { OperatorClient } from './client';
import { operatorFailure, type OperatorFailure } from './failure';
import { FailureNotice, Timestamp } from './records';
import { SpendField } from './SpendField';

const closureLabels: Readonly<Record<SpendClosure['reason'], string>> = {
  spend_warning: 'Spend warning',
  spend_stop: 'Spend stop',
  metering_unavailable: 'Metering unavailable',
};

const observationFailures: Readonly<Record<string, string>> = {
  invalid_request:
    'The observation was refused. Check its time and units; a different observation for the same time may already exist.',
  model_budget_writes_unavailable:
    'Recording cloud observations is not available yet. Your entry has been retained.',
  platform_spend_unavailable:
    'Platform spend is temporarily unavailable. Your entry has been retained; check the status before recording again.',
};

const RFC3339 = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$/;

function observationErrors(observedThrough: string, units: string) {
  const errors: { observedThrough?: string; units?: string } = {};
  if (!RFC3339.test(observedThrough) || Number.isNaN(Date.parse(observedThrough)))
    errors.observedThrough =
      'Enter an RFC 3339 time with a time zone, such as 2026-11-30T00:00:00Z.';
  if (!/^(0|[1-9]\d*)$/.test(units) || !Number.isSafeInteger(Number(units)))
    errors.units = 'Enter a whole number of accounting units.';
  return errors;
}

/** Read-only platform spend state plus explicit cloud observation entry. */
export function SpendStatus({
  client,
  disabled,
}: {
  readonly client: OperatorClient;
  readonly disabled: boolean;
}) {
  const [spend, setSpend] = useState<PlatformSpend>();
  const [failure, setFailure] = useState<OperatorFailure>();
  const [busy, setBusy] = useState(false);
  const [observedThrough, setObservedThrough] = useState('');
  const [units, setUnits] = useState('');
  const [errors, setErrors] = useState<ReturnType<typeof observationErrors>>({});
  const [recorded, setRecorded] = useState(false);
  const pending = useRef(false);
  const run = async (operation: () => Promise<PlatformSpend>, record: boolean) => {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    setFailure(undefined);
    setRecorded(false);
    try {
      setSpend(await operation());
      if (record) {
        setRecorded(true);
        setObservedThrough('');
        setUnits('');
      }
    } catch (error) {
      const result = operatorFailure(error);
      const message = record ? observationFailures[result.code ?? ''] : undefined;
      setFailure(message === undefined ? result : { ...result, message });
    } finally {
      pending.current = false;
      setBusy(false);
    }
  };
  const submit = () => {
    const next = observationErrors(observedThrough, units);
    setErrors(next);
    if (next.observedThrough !== undefined || next.units !== undefined) return;
    // An uncertain write is never retried automatically; the operator checks status first.
    void run(
      () =>
        client.admin.recordSpendObservation({
          source: 'cloud',
          observed_through: observedThrough,
          units: Number(units),
        }),
      true,
    );
  };
  return (
    <div>
      <h4>Platform spend status</h4>
      <div className="operator-actions">
        <Button disabled={busy} onClick={() => void run(() => client.admin.readSpend(), false)}>
          Check platform spend
        </Button>
      </div>
      {busy ? <p role="status">Checking platform spend…</p> : null}
      {spend === undefined ? null : <SpendState spend={spend} />}
      {spend?.metering_enabled === true ? (
        <div>
          <h4>Record cloud observation</h4>
          <p className="operator-notice">
            Record the cloud cost observed through a time in this period. Observations are
            immutable; a conflicting observation for the same time is refused.
          </p>
          <div className="operator-settings-grid">
            <SpendField
              label="Observed through (RFC 3339)"
              value={observedThrough}
              disabled={disabled || busy}
              error={errors.observedThrough}
              onChange={setObservedThrough}
            />
            <SpendField
              label="Cloud units"
              value={units}
              numeric
              disabled={disabled || busy}
              error={errors.units}
              onChange={setUnits}
            />
          </div>
          <div className="operator-actions">
            <Button disabled={disabled || busy} onClick={submit}>
              Record cloud observation
            </Button>
          </div>
          {recorded ? <p role="status">Cloud observation recorded.</p> : null}
        </div>
      ) : null}
      <FailureNotice failure={failure} />
    </div>
  );
}

function SpendState({ spend }: { readonly spend: PlatformSpend }) {
  if (!spend.metering_enabled)
    return (
      <p role="status">
        Platform spend metering is not enabled. Configure cloud metering in the platform period to
        enable it.
      </p>
    );
  const ledger = spend.ledger;
  const closures = ledger?.closures ?? [];
  return (
    <div className="operator-detail">
      <p role="status">
        {closures.length === 0
          ? 'Platform spend is open. New spend commitments are admitted.'
          : 'Platform spend is closed to new commitments.'}
      </p>
      {closures.length === 0 ? null : (
        <ul>
          {closures.map((closure) => (
            <li key={`${closure.reason}:${closure.since}`}>
              {closureLabels[closure.reason]} since <Timestamp value={closure.since} />
            </li>
          ))}
        </ul>
      )}
      <dl>
        {spend.levels === undefined ? null : (
          <>
            <dt>Warning level</dt>
            <dd>{spend.levels.warning}</dd>
            <dt>Stop level</dt>
            <dd>{spend.levels.stop}</dd>
            <dt>Disposable level</dt>
            <dd>{spend.levels.disposable}</dd>
          </>
        )}
        {spend.committed === undefined ? null : (
          <>
            <dt>Committed</dt>
            <dd>{spend.committed}</dd>
          </>
        )}
        {ledger === undefined ? null : (
          <>
            <dt>Period</dt>
            <dd>
              <Timestamp value={ledger.period_start} /> – <Timestamp value={ledger.period_end} />
            </dd>
            <dt>Latest cloud observation</dt>
            <dd>
              {ledger.cloud === undefined ? (
                'None recorded in this period'
              ) : (
                <>
                  {ledger.cloud.units} units through{' '}
                  <Timestamp value={ledger.cloud.observed_through} />
                  , recorded <Timestamp value={ledger.cloud.recorded_at} />
                </>
              )}
            </dd>
          </>
        )}
      </dl>
    </div>
  );
}
