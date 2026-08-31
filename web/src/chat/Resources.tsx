import { useState } from 'react';
import { fullTimestamp } from '../sessions/sessionTime';
import { IconCloseOutline16 } from '../ui/icons';
import type { CpuPressure, MemoryPressure, Resources as ResourceReading } from '@chronoai/cma-sdk';
import './Resources.css';

interface ResourcesProps {
  /** Whether the session was granted `resources` at all. */
  readonly granted: boolean;
  /** Whether this session's runtime image said it cannot report. */
  readonly unreported?: boolean;
  /** What has arrived, when anything has. */
  readonly resources?: ResourceReading;
  /** The open processor-pressure episode, which gives the CPU row its failure wording. */
  readonly cpuPressure?: CpuPressure;
  /** The open memory-pressure episode, which gives the memory row its failure wording. */
  readonly memoryPressure?: MemoryPressure;
}

interface PressureNoticeProps {
  readonly label: 'CPU' | 'Memory';
  readonly since: string;
  readonly used: number;
  readonly limit: number;
  readonly format: (value: number) => string;
}

interface BoundedRowProps {
  readonly label: string;
  readonly used: number | undefined;
  readonly limit: number | undefined;
  readonly format: (value: number) => string;
  readonly formatLimit?: (value: number) => string;
  readonly measuring?: boolean;
  readonly pressure?: boolean;
}

const BYTE_UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'] as const;

/** Render one exact byte count without implying precision the reading lacks. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes.toLocaleString('en')} B`;
  }
  const unit = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), BYTE_UNITS.length - 1);
  const amount = bytes / 1024 ** unit;
  const figure =
    amount < 100 ? amount.toFixed(1).replace(/\.0$/, '') : Math.round(amount).toLocaleString('en');
  return `${figure} ${BYTE_UNITS[unit]}`;
}

/** Thousandths of a core, in the vocabulary container limits use. */
export function formatCpu(millis: number): string {
  return `${String(millis)}m`;
}

/** Memory in the same mebibyte unit used to size a session. */
export function formatMemory(bytes: number): string {
  if (bytes < 1024 ** 2) {
    return formatBytes(bytes);
  }
  const mebibytes = bytes / 1024 ** 2;
  const figure =
    mebibytes < 100 ? mebibytes.toFixed(1).replace(/\.0$/, '') : String(Math.round(mebibytes));
  return `${figure} MiB`;
}

function percentage(used: number, limit: number): number {
  if (limit === 0) {
    return used === 0 ? 0 : 100;
  }
  return Math.min(100, Math.round((used / limit) * 100));
}

function BoundedRow({
  label,
  used,
  limit,
  format,
  formatLimit = format,
  measuring,
  pressure,
}: BoundedRowProps) {
  if (used === undefined) {
    return (
      <div className="resources__row">
        <span className="resources__figure">
          {label} · {measuring === true ? 'measuring' : 'not reported'}
        </span>
      </div>
    );
  }

  const shownLabel = pressure === true ? `${label} under pressure` : label;
  if (limit === undefined) {
    return (
      <div className="resources__row">
        <span className="resources__figure">
          {shownLabel} · {format(used)}, no ceiling reported
        </span>
      </div>
    );
  }

  const percent = percentage(used, limit);
  return (
    <div className="resources__row">
      <span className="resources__figure">
        {shownLabel} · {format(used)} / {formatLimit(limit)}
      </span>
      <span
        className="resources__track"
        data-pressure={pressure === true ? 'true' : undefined}
        role="progressbar"
        aria-label={shownLabel}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
      >
        <span className="resources__fill" style={{ width: `${String(percent)}%` }} />
      </span>
    </div>
  );
}

/** A session's live CPU, memory, and unbounded network figures. */
export function Resources({
  granted,
  unreported,
  resources,
  cpuPressure,
  memoryPressure,
}: ResourcesProps) {
  if (!granted) {
    return <span className="resources__status">Resources not reported for this session</span>;
  }
  if (unreported === true) {
    return (
      <span className="resources__status">
        This session's runtime cannot report what it is using
      </span>
    );
  }
  if (resources === undefined) {
    return <span className="resources__status">Resources · waiting</span>;
  }

  const network =
    resources.receivedBytes === undefined || resources.sentBytes === undefined
      ? 'not reported'
      : `↓ ${formatBytes(resources.receivedBytes)} · ↑ ${formatBytes(resources.sentBytes)}`;

  return (
    <div className="resources" aria-label="Resources">
      <BoundedRow
        label="Memory"
        used={resources.memoryBytesUsed}
        limit={resources.memoryBytesLimit}
        format={formatMemory}
        pressure={memoryPressure !== undefined}
      />
      <BoundedRow
        label="CPU"
        used={resources.cpuMillisUsed}
        limit={resources.cpuMillisLimit}
        format={formatCpu}
        measuring
        pressure={cpuPressure !== undefined}
      />
      <div className="resources__row">
        <span className="resources__figure">Network · {network}</span>
      </div>
    </div>
  );
}

/** One actionable, durable episode at the head of the reading column. */
function PressureNotice({ label, since, used, limit, format }: PressureNoticeProps) {
  const [dismissedSince, setDismissedSince] = useState<string | undefined>();
  if (since === dismissedSince) {
    return null;
  }

  const opened = Date.parse(since);
  const openedAt = Number.isNaN(opened) ? since : fullTimestamp(opened);
  const lowerLabel = label.toLowerCase();
  return (
    <div className="notice notice--fail chat__pressure" role="alert">
      <div className="chat__pressure-copy">
        <p className="notice__title">{label} pressure is high</p>
        <p className="notice__detail">
          {format(used)} used of {format(limit)}. Opened at <time dateTime={since}>{openedAt}</time>
          .
        </p>
      </div>
      <button
        type="button"
        className="chat__pressure-dismiss"
        aria-label={`Dismiss ${lowerLabel} pressure notice`}
        title={`Dismiss ${lowerLabel} pressure notice`}
        onClick={() => {
          setDismissedSince(since);
        }}
      >
        <IconCloseOutline16 size={14} />
      </button>
    </div>
  );
}

/** The actionable, durable memory episode at the head of the reading column. */
export function MemoryPressureNotice({ pressure }: { readonly pressure?: MemoryPressure }) {
  return pressure === undefined ? null : (
    <PressureNotice
      label="Memory"
      since={pressure.since}
      used={pressure.usedBytes}
      limit={pressure.limitBytes}
      format={formatBytes}
    />
  );
}

/** The actionable, durable processor episode at the head of the reading column. */
export function CpuPressureNotice({ pressure }: { readonly pressure?: CpuPressure }) {
  return pressure === undefined ? null : (
    <PressureNotice
      label="CPU"
      since={pressure.since}
      used={pressure.usedMillis}
      limit={pressure.limitMillis}
      format={formatCpu}
    />
  );
}
