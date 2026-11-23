import {
  createInitial,
  readInitial,
  latestInitial,
  type UsageInitialCreate,
  type UsageInitialViewInput,
  type UsageInitialProjection,
} from './initial.js';
export type {
  UsageInitialCreate,
  UsageInitialViewInput,
  UsageInitialProjection,
  UsageInitialReport,
  UsageInitialView,
  UsageInitialData,
} from './initial.js';
export { isUsageInitialUnavailable } from './initial.js';
import { latestView, type UsageMetricViewQuery } from './view.js';
export { isUsageViewUnavailable, type UsageMetricViewQuery, type UsageMetricView } from './view.js';
import type { ResolvedConfig } from '../config.js';
import { ProtocolError, ValidationError } from '../errors.js';
import type { OperationOptions } from '../http.js';
import { query, read, segment } from '../resources.js';
import * as schema from './schema.js';
import { ownerAllocations, type UsageOwnerAllocationQuery } from './owner-allocation.js';
export type {
  UsageOwnerAllocationQuery,
  UsageOwnerAllocations,
  UsageOwnerAllocation,
} from './owner-allocation.js';
import { filterOptions, type UsageFilterOptionsQuery } from './options.js';
export type { UsageFilterOptionsQuery, UsageFilterOptions } from './options.js';
export type {
  UsageMetricQuantity,
  UsageMetricOwners,
  UsageMetricReport,
  UsageMetricRow,
  UsageMetricPage,
  UsageMetricSeries,
  UsageMetricStatus,
} from './schema.js';

export interface UsageMetricQuery {
  readonly from?: number;
  readonly to?: number;
  readonly step_seconds?: number;
  readonly filters?: {
    readonly owner?: string | null;
    readonly statuses?: readonly schema.UsageMetricStatus[];
    readonly runtime_version?: string | null;
    readonly sandbox_id?: string | null;
    readonly agent_id?: string | null;
    readonly search?: string;
  };
}
export type UsageMetricSort =
  | 'name'
  | 'owner'
  | 'status'
  | 'last_activity_at'
  | 'cpu_mean_cores'
  | 'memory_mean_bytes'
  | 'disk_latest_bytes'
  | 'allocated_cpu_core_hours'
  | 'allocated_memory_gib_hours'
  | 'allocated_disk_gib_hours';
export interface UsageMetricPageOptions extends OperationOptions {
  readonly limit?: number;
  readonly cursor?: string;
  readonly sort?: UsageMetricSort;
  readonly order?: 'asc' | 'desc';
}
export interface UsageMetricSeriesOptions extends OperationOptions {
  readonly metric: 'cpu' | 'memory' | 'disk';
  readonly group_by?: 'total' | 'owner' | 'sandbox';
  readonly series_id?: string;
  readonly top?: number;
}
/** Measured CPU/working-set/PVC history. Estimates never change allocated billing. */
export class SandboxMetrics {
  createInitial(input: UsageInitialCreate, options: OperationOptions = {}) {
    return createInitial(this.config, this.admin, input, options);
  }
  initialReport(id: string, projection: UsageInitialProjection, options: OperationOptions = {}) {
    return readInitial(this.config, this.admin, id, projection, options);
  }
  latestInitial(input: UsageInitialViewInput, options: OperationOptions = {}) {
    return latestInitial(this.config, this.admin, input, options);
  }

  /** Return a committed snapshot immediately; refreshes run in leased workers. */
  latest(input: UsageMetricViewQuery = {}, options: OperationOptions = {}) {
    return latestView(this.config, this.admin, input, options);
  }

  /** Bounded administrator discovery independent of report readiness. */
  options(query: UsageFilterOptionsQuery) {
    return filterOptions(this.config, this.admin, query);
  }
  /** Paged, globally ranked allocation across the same report; administrator only. */
  ownerAllocations(id: string, options: UsageOwnerAllocationQuery) {
    return ownerAllocations(this.config, this.admin, id, options);
  }
  constructor(
    readonly config: ResolvedConfig,
    readonly admin = false,
  ) {}
  private get base() {
    return `/api/v1/${this.admin ? 'admin/' : ''}usage/metrics/reports`;
  }
  create(
    input: UsageMetricQuery = {},
    options: OperationOptions = {},
  ): Promise<schema.UsageMetricReport> {
    if (!this.admin && input.filters?.owner != null)
      throw new ValidationError('owner filters require administrator reporting');
    return read(this.config, this.base, schema.report, {
      ...options,
      method: 'POST',
      body: input,
      expectedStatus: [200, 202],
    });
  }
  report(id: string, options: OperationOptions = {}): Promise<schema.UsageMetricReport> {
    return read(
      this.config,
      `${this.base}/${segment(id)}`,
      (value) => {
        const report = schema.report(value);
        if (report.report_id !== id) throw new ProtocolError('report identity did not match');
        return report;
      },
      options,
    );
  }
  sandboxes(id: string, options: UsageMetricPageOptions = {}): Promise<schema.UsageMetricPage> {
    if (
      options.limit !== undefined &&
      (!Number.isInteger(options.limit) || options.limit < 1 || options.limit > 200)
    )
      throw new ValidationError('limit must be from 1 through 200');
    return read(
      this.config,
      query(`${this.base}/${segment(id)}/sandboxes`, {
        limit: options.limit,
        cursor: options.cursor,
        sort: options.sort,
        order: options.order,
      }),
      (value) => {
        const page = schema.page(value);
        if (page.report_id !== id) throw new ProtocolError('page report identity did not match');
        return page;
      },
      options,
    );
  }
  /** Search owners across the complete frozen administrator cohort. */
  owners(
    id: string,
    options: OperationOptions & {
      readonly search?: string;
      readonly cursor?: string;
      readonly limit?: number;
    } = {},
  ): Promise<schema.UsageMetricOwners> {
    if (!this.admin) throw new ValidationError('owner search requires administrator reporting');
    return read(
      this.config,
      query(`${this.base}/${segment(id)}/owners`, {
        search: options.search,
        cursor: options.cursor,
        limit: options.limit,
      }),
      (value) => {
        const owners = schema.owners(value);
        if (owners.report_id !== id) throw new ProtocolError('owner report identity did not match');
        return owners;
      },
      options,
    );
  }
  sandbox(
    report: string,
    sandbox: string,
    options: OperationOptions = {},
  ): Promise<schema.UsageMetricRow> {
    return read(
      this.config,
      `${this.base}/${segment(report)}/sandboxes/${segment(sandbox)}`,
      (value) => {
        const row = schema.row(value);
        if (row.sandbox_id !== sandbox) throw new ProtocolError('Sandbox identity did not match');
        return row;
      },
      options,
    );
  }
  series(id: string, options: UsageMetricSeriesOptions): Promise<schema.UsageMetricSeries> {
    return read(
      this.config,
      query(`${this.base}/${segment(id)}/series`, {
        metric: options.metric,
        group_by: options.group_by,
        series_id: options.series_id,
        top: options.top,
      }),
      (value) => {
        const series = schema.series(value);
        if (series.report_id !== id || series.metric !== options.metric)
          throw new ProtocolError('series identity did not match');
        return series;
      },
      options,
    );
  }
}
