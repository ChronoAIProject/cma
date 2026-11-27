import { useCallback, useMemo, useState, type ReactNode } from 'react';
import type { Cma, UsageMetricRow } from '@cma/client';
import { UsageReportProgress } from './UsageReportProgress';
import { useWarmUsageRanges } from './warmRanges';
import { UsageOverviewHeader } from './UsageOverviewHeader';
import { Picker } from '../../ui/Picker';
import { MetricCharts } from './MetricCharts';
import { MetricDetail } from './MetricDetail';
import { MetricFilters } from './MetricFilters';
import { OpsMetricFilters } from './OpsMetricFilters';
import { MetricSummary } from './MetricSummary';
import { UsageAllocationOverview } from './UsageAllocationOverview';
import { MetricTable } from './MetricTable';
import { MetricsNotice } from './MetricsNotice';
import { useUsageReport, useUsageVisible } from './useReport';
import {
  UsageReportOwnerContext,
  anonymousReportOwner,
  type UsageReportOwner,
} from './reportOwner';
import type { MetricsFailure } from './failure';
import type { Group, Metric, MetricsClient, Selection } from './model';
import { allocationSelection } from './selection';
import { SelectedResourceLabels } from './selectedResourceLabel';
import './Metrics.css';

export function MetricsDashboard(props: Parameters<typeof MetricsDashboardContent>[0]) {
  return (
    <UsageReportOwnerContext value={props.principal ?? anonymousReportOwner}>
      <SelectedResourceLabels
        key={JSON.stringify([props.principal?.cacheKey, props.principal?.principal, props.admin])}
        scope={props.api}
      >
        <MetricsDashboardContent {...props} />
      </SelectedResourceLabels>
    </UsageReportOwnerContext>
  );
}

function MetricsDashboardContent({
  api,
  history,
  admin,
  principal = anonymousReportOwner,
  forceOnMount = false,
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
  readonly principal?: UsageReportOwner;
  readonly forceOnMount?: boolean;
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
  const state = useUsageReport(
    api,
    effectiveSelection,
    admin,
    active,
    principal,
    forceOnMount,
    measured,
    true,
  );
  useWarmUsageRanges(
    api,
    effectiveSelection,
    admin,
    principal,
    visible && !state.error && Boolean(state.report || state.pending),
  );
  const [group, setGroup] = useState<Group>('total');
  const [metric, setMetric] = useState<Metric>('cpu');
  const [allocationMetric, setAllocationMetric] = useState<Metric>('cpu');
  const [detail, setDetail] = useState<{ report: string; sandbox: string }>();
  const [blocked, setBlocked] = useState<{ report: string; failure: MetricsFailure }>();
  const preview = state.report ? undefined : state.fallback;
  const displayedReport = state.report ?? preview?.report;
  const reportId = displayedReport?.report_id;
  const initialMatchesSelection =
    !preview &&
    state.initial &&
    state.initialProjection?.first_page?.sort === effectiveSelection.sort &&
    state.initialProjection?.first_page?.order === effectiveSelection.order;
  const initialPage = initialMatchesSelection ? state.initial?.rows : undefined;
  const initialCharts = initialMatchesSelection ? state.initial?.charts : undefined;
  const onFailure = useCallback(
    (failure: MetricsFailure) => {
      if (reportId) setBlocked({ report: reportId, failure });
    },
    [reportId],
  );
  const failure = blocked?.report === reportId ? blocked?.failure : undefined;
  const report = !failure ? displayedReport : undefined;
  const safeActions = actions
    ? (row: UsageMetricRow) => actions(row, state.busy || state.stale || !!state.error || !!preview)
    : undefined;
  const resourceDetails = report && (
    <>
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
          active={visible}
          initialCharts={initialCharts}
          onFailure={onFailure}
        />
      )}
    </>
  );
  return (
    <div
      className="usage-metrics"
      aria-label={admin ? 'Operations resource dashboard' : 'Your usage resources'}
      data-usage-requested-range={effectiveSelection.range}
      data-usage-state={
        preview ? 'preview' : state.error || failure ? 'error' : report ? 'committed' : 'pending'
      }
    >
      {admin ? (
        <OpsMetricFilters
          api={api}
          selection={effectiveSelection}
          active={visible}
          onApply={onSelection}
        />
      ) : (
        <MetricFilters
          api={api}
          selection={effectiveSelection}
          admin={admin}
          active={visible}
          onApply={onSelection}
          sandboxReport={
            !state.busy &&
            !state.stale &&
            !preview &&
            !state.error &&
            !failure &&
            report?.scope.admin === false
              ? report
              : undefined
          }
          selectionUnavailable={Boolean(
            state.error ||
            failure ||
            (!state.busy &&
              !state.stale &&
              !preview &&
              report &&
              (report.query.filters.sandbox_id !== effectiveSelection.sandbox ||
                Number(report.summary?.sandbox_count) === 0)),
          )}
          agentPicker={
            agentPicker
              ? (draft, update) => {
                  const ownsSelection =
                    !admin &&
                    !state.busy &&
                    !state.stale &&
                    !preview &&
                    !state.error &&
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
      )}
      <UsageOverviewHeader
        report={report}
        generatedAt={state.generatedAt ?? preview?.generatedAt}
        busy={state.busy && !preview}
        refreshing={!preview && state.refreshing}
        stale={!preview && state.stale}
        onRefresh={state.refresh}
      />
      <UsageReportProgress
        range={effectiveSelection.range}
        previewRange={report ? preview?.range : undefined}
        busy={state.busy || state.retrying}
        pending={Boolean(state.pending)}
        retrying={state.retrying}
        startedAt={state.startedAt}
        nextCheckAt={state.nextCheckAt}
        active={visible}
      />
      <MetricsNotice failure={state.error ?? failure} onRetry={state.refresh} />
      {selection.agent && (
        <p>
          Shared Sandbox usage: Agent selection resolves to its owned Sandbox. CPU, memory and disk
          are shared with its other Agents.
        </p>
      )}
      {report && (
        <section className="usage-report" aria-busy={(state.busy && !preview) || undefined}>
          {!admin && <UsageAllocationOverview value={report.summary?.allocated_period ?? null} />}
          {admin && resourceDetails}
          <MetricTable
            key={report.report_id}
            api={api}
            reportId={report.report_id}
            initialPage={initialPage}
            reportCoverage={report.summary?.allocated_period}
            sort={effectiveSelection.sort}
            order={effectiveSelection.order}
            admin={admin}
            active={visible}
            measured={measured}
            allocationMetric={allocationMetric}
            onAllocationMetricChange={setAllocationMetric}
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
          {!admin && resourceDetails}
          {detail?.report === report.report_id && (
            <MetricDetail
              key={report.report_id + detail.sandbox}
              api={api}
              history={history}
              reportId={report.report_id}
              sandboxId={detail.sandbox}
              measured={measured}
              active={visible}
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
