import { useCallback, useMemo, useState, type ReactNode } from 'react';
import type { Cma, UsageMetricRow } from '@chronoai/cma-sdk';
import { Skeleton } from '../../ui/Skeleton';
import { UsageOverviewHeader } from './UsageOverviewHeader';
import { Picker } from '../../ui/Picker';
import { MetricCharts } from './MetricCharts';
import { MetricDetail } from './MetricDetail';
import { MetricFilters } from './MetricFilters';
import { MetricSummary } from './MetricSummary';
import { MetricTable } from './MetricTable';
import { MetricsNotice } from './MetricsNotice';
import { useUsageReport, useUsageVisible } from './useReport';
import type { MetricsFailure } from './failure';
import type { Group, Metric, MetricsClient, Selection } from './model';
import { allocationSelection } from './selection';
import './Metrics.css';

export function MetricsDashboard({
  api,
  history,
  admin,
  selection,
  onSelection,
  active = true,
  measured = false,
  actions,
  agentPicker,
}: {
  readonly api: MetricsClient;
  readonly history: Pick<Cma['usage'], 'events'>;
  readonly admin: boolean;
  readonly selection: Selection;
  readonly onSelection: (value: Selection) => void;
  readonly active?: boolean;
  readonly measured?: boolean;
  readonly actions?: (row: UsageMetricRow, disabled: boolean) => ReactNode;
  readonly agentPicker?: (draft: Selection, update: (value: Selection) => void) => ReactNode;
}) {
  const effectiveSelection = useMemo(
    () => (measured ? selection : allocationSelection(selection)),
    [measured, selection],
  );
  const visible = useUsageVisible(active);
  const state = useUsageReport(api, effectiveSelection, admin, active);
  const [group, setGroup] = useState<Group>('total');
  const [metric, setMetric] = useState<Metric>('cpu');
  const [detail, setDetail] = useState<{ report: string; sandbox: string }>();
  const [blocked, setBlocked] = useState<{ report: string; failure: MetricsFailure }>();
  const reportId = state.report?.report_id;
  const onFailure = useCallback(
    (failure: MetricsFailure) => {
      if (reportId) setBlocked({ report: reportId, failure });
    },
    [reportId],
  );
  const failure = blocked?.report === reportId ? blocked?.failure : undefined;
  const report = !failure ? state.report : undefined;
  const safeActions = actions
    ? (row: UsageMetricRow) => actions(row, state.busy || state.stale || !!state.error)
    : undefined;
  return (
    <div
      className="usage-metrics"
      aria-label={admin ? 'Operations resource dashboard' : 'Your usage resources'}
    >
      <MetricFilters
        api={api}
        selection={effectiveSelection}
        admin={admin}
        active={visible}
        onApply={onSelection}
        agentPicker={
          agentPicker
            ? (draft, update) => {
                const ownsSelection =
                  !admin &&
                  !state.busy &&
                  !state.stale &&
                  report?.scope.admin === false &&
                  report.query.filters.sandbox_id === draft.sandbox &&
                  Number(report.summary?.sandbox_count) > 0;
                return ownsSelection ? (
                  agentPicker(draft, update)
                ) : (
                  <p>Choose a Sandbox and apply filters to select an Agent.</p>
                );
              }
            : undefined
        }
      />
      <UsageOverviewHeader report={report} busy={state.busy} onRefresh={state.refresh} />
      {state.busy && (
        <p role="status">{state.pending ? 'Preparing usage report…' : 'Loading usage report…'}</p>
      )}
      {state.stale && (
        <p role="status">Showing the previous selection. The selected report is not ready.</p>
      )}
      <MetricsNotice failure={state.error ?? failure} onRetry={state.refresh} />
      {selection.agent && (
        <p>
          Shared Sandbox usage: Agent selection resolves to its owned Sandbox. CPU, memory and disk
          are shared with its other Agents.
        </p>
      )}
      {state.busy && !report && (
        <div className="usage-loading" aria-hidden="true">
          <div className="usage-metric-cards">
            {[0, 1, 2].map((item) => (
              <div className="usage-metric-card" key={item}>
                <Skeleton width="40%" />
                <Skeleton height={32} width="70%" />
              </div>
            ))}
          </div>
          <Skeleton height={240} radius={12} />
        </div>
      )}
      {report && (
        <section className="usage-report" aria-busy={state.busy || undefined}>
          <MetricSummary
            report={report}
            measured={measured}
            selectedMetric={admin ? undefined : metric}
            onMetric={admin ? undefined : setMetric}
          />
          {admin && measured && (
            <Picker
              label="Chart grouping"
              value={group}
              options={[
                { value: 'total', label: 'Total' },
                { value: 'owner', label: 'By owner' },
                { value: 'sandbox', label: 'By Sandbox' },
              ]}
              onChange={setGroup}
            />
          )}
          {measured && (
            <MetricCharts
              api={api}
              report={report}
              group={admin ? group : 'total'}
              metric={admin ? undefined : metric}
              active={visible && !state.busy}
              onFailure={onFailure}
            />
          )}
          <MetricTable
            key={`${report.report_id}:${effectiveSelection.sort}:${effectiveSelection.order}`}
            api={api}
            reportId={report.report_id}
            sort={effectiveSelection.sort}
            order={effectiveSelection.order}
            admin={admin}
            active={visible && !state.busy}
            measured={measured}
            focusedMetric={admin || !measured ? undefined : metric}
            onSort={(sort) =>
              onSelection({
                ...effectiveSelection,
                sort,
                order:
                  effectiveSelection.sort === sort && effectiveSelection.order === 'asc'
                    ? 'desc'
                    : 'asc',
              })
            }
            onDetail={(row) => setDetail({ report: report.report_id, sandbox: row.sandbox_id })}
            onFailure={onFailure}
            actions={safeActions}
          />
          {detail?.report === report.report_id && (
            <MetricDetail
              key={report.report_id + detail.sandbox}
              api={api}
              history={history}
              reportId={report.report_id}
              sandboxId={detail.sandbox}
              measured={measured}
              active={visible && !state.busy}
              admin={admin}
              onClose={() => setDetail(undefined)}
              onOwner={(owner) => onSelection({ ...selection, owner, sandbox: '', agent: '' })}
              onFailure={onFailure}
              actions={safeActions}
            />
          )}
        </section>
      )}
    </div>
  );
}
