import { useRef, useState } from 'react';
import type { UsageMetricRow } from '@cma/client';
import { Button } from '../ui/Button';
import { Presence } from '../ui/Presence';
import { RiskConfirmation } from '../ui/RiskConfirmation';
import { MetricsDashboard } from '../usage/metrics/MetricsDashboard';
import { useMeasuredUsage } from '../usage/metrics/useMeasuredUsage';
import { useUsageSelection } from '../usage/metrics/selection';
import { type UsageReportOwner } from '../usage/metrics/reportOwner';
import { SandboxDetail } from './SandboxDetail';
import { operatorFailure, type OperatorFailure } from './failure';
import type { OperatorClient } from './client';

export function Sandboxes({
  client,
  canControl,
  principal,
}: {
  readonly client: OperatorClient;
  readonly canControl: boolean;
  readonly principal?: UsageReportOwner;
}) {
  const measured = useMeasuredUsage(client.readMeasuredUsage);
  const [selection, onSelection] = useUsageSelection(true);
  const [revision, setRevision] = useState(0);
  const [detail, setDetail] = useState<string>();
  const [target, setTarget] =
    useState<Pick<UsageMetricRow, 'sandbox_id' | 'name' | 'status' | 'owner'>>();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<OperatorFailure>();
  const [uncertain, setUncertain] = useState(false);
  const submitting = useRef(false);
  const retire = async () => {
    if (target === undefined || !canControl || submitting.current) return;
    submitting.current = true;
    setBusy(true);
    setFailure(undefined);
    try {
      if (uncertain) {
        const current = await client.admin.readSandbox(target.sandbox_id);
        setTarget({ ...target, status: current.sandbox.status });
        setUncertain(false);
        setRevision((value) => value + 1);
        if (current.sandbox.status === 'retiring' || current.sandbox.status === 'retired')
          setTarget(undefined);
      } else {
        await client.admin.retireSandbox(target.sandbox_id);
        setTarget(undefined);
        setRevision((value) => value + 1);
      }
    } catch (error) {
      const problem = operatorFailure(error);
      setFailure(problem);
      setUncertain((previous) => previous || problem.uncertain);
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  };
  return (
    <section aria-label="Sandbox resource dashboard">
      <MetricsDashboard
        key={revision}
        api={client.adminUsageMetrics}
        history={client.usage}
        admin
        principal={principal}
        forceOnMount={revision > 0}
        measured={measured}
        selection={selection}
        onSelection={onSelection}
        actions={(row, disabled) => (
          <>
            <Button
              size="sm"
              aria-label={`Runtime details for ${row.name}`}
              onClick={() => setDetail(row.sandbox_id)}
            >
              Runtime details
            </Button>
            <Button
              size="sm"
              variant="danger"
              title={
                !canControl ? 'Cross-owner control is required to retire a Sandbox.' : undefined
              }
              disabled={
                disabled || !canControl || row.status === 'retired' || row.status === 'retiring'
              }
              onClick={() => {
                setFailure(undefined);
                setUncertain(false);
                setTarget(row);
              }}
            >
              Retire
            </Button>
            {!canControl && (
              <span
                className="operator-notice"
                title="Cross-owner control is required to retire a Sandbox."
              >
                Control required
              </span>
            )}
          </>
        )}
      />
      {detail === undefined ? null : (
        <SandboxDetail client={client} id={detail} onClose={() => setDetail(undefined)} />
      )}
      <Presence>
        {target === undefined || !canControl ? null : (
          <RiskConfirmation
            open
            title={`Retire ${target.name}`}
            description={`Owner: ${target.owner?.display_name ?? 'Name unavailable'}. Retiring Sandbox ${target.sandbox_id} stops its Agents and releases its runtime. It cannot be resumed. The operator action is audited.`}
            acknowledgeLabel="I understand this Sandbox cannot be resumed."
            confirmLabel={uncertain ? 'Check current state' : 'Retire Sandbox'}
            busy={busy}
            dismissible={!busy}
            failure={
              failure === undefined
                ? undefined
                : `${failure.message}${failure.code === undefined ? '' : ` (${failure.code})`}`
            }
            status={
              uncertain
                ? 'The previous outcome is unknown. Only a read will be sent next.'
                : busy
                  ? 'Submitting'
                  : undefined
            }
            onCancel={() => setTarget(undefined)}
            onConfirm={() => void retire()}
          />
        )}
      </Presence>
    </section>
  );
}
