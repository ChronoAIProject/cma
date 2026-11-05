import { useCallback, useState, type ReactNode } from 'react';
import type { Cma, UsageMetricRow } from '@chronoai/cma-sdk';
import { Button } from '../../ui/Button';
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
import './Metrics.css';

export function MetricsDashboard({
  api,
  history,
  admin,
  selection,
  onSelection,
  active = true,
  actions,
  agentPicker,
}: {
  readonly api: MetricsClient;
  readonly history: Pick<Cma['usage'], 'events'>;
  readonly admin: boolean;
  readonly selection: Selection;
  readonly onSelection: (value: Selection) => void;
  readonly active?: boolean;
  readonly actions?: (row: UsageMetricRow, disabled: boolean) => ReactNode;
  readonly agentPicker?: (draft: Selection, update: (value: Selection) => void) => ReactNode;
}) {
  const visible = useUsageVisible(active);
  const state = useUsageReport(api, selection, admin, active);
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
      aria-label={admin ? 'Operations resource dashboard' : 'Your measured usage'}
    >
      <MetricFilters
        api={api}
        selection={selection}
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
                  <p>Apply one of your Sandbox selections to optionally choose an Agent.</p>
                );
              }
            : undefined
        }
      />
      <div className="usage-metric-actions">
        <Button size="sm" disabled={state.busy} onClick={state.refresh}>
          Refresh usage
        </Button>
        {state.busy && (
          <p role="status">{state.pending ? 'Preparing usage report…' : 'Loading usage report…'}</p>
        )}
      </div>
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
      {report && (
        <section aria-busy={state.busy || undefined}>
          <MetricSummary
            report={report}
            selectedMetric={admin ? undefined : metric}
            onMetric={admin ? undefined : setMetric}
          />
          {admin && (
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
          <MetricCharts
            api={api}
            report={report}
            group={admin ? group : 'total'}
            metric={admin ? undefined : metric}
            active={visible && !state.busy}
            onFailure={onFailure}
          />
          <MetricTable
            key={`${report.report_id}:${selection.sort}:${selection.order}`}
            api={api}
            reportId={report.report_id}
            sort={selection.sort}
            order={selection.order}
            admin={admin}
            active={visible && !state.busy}
            focusedMetric={admin ? undefined : metric}
            onSort={(sort) =>
              onSelection({
                ...selection,
                sort,
                order: selection.sort === sort && selection.order === 'asc' ? 'desc' : 'asc',
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
