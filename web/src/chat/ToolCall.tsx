import type { ToolCallEntry } from './conversation';
import type { DetailsSection } from './DetailsPanel';
import './ToolCall.css';
import { StateDot } from '../ui/StateDot';

/**
 * A tool call, as structure — `Web_Style.md` §18.2.
 *
 * This is the section that pays for the plane. The whole reason conversation
 * mode exists is that these are semantics rather than scrollback, and a tool
 * call rendered as a wall of text throws away the thing the milestone was
 * built to obtain.
 *
 * Collapsed by default, and **the collapsed row identifies the call without
 * being opened**: the kernel's own word for what kind of tool it is, and what
 * it acted on. A row reading only "tool call" is a row somebody has to expand
 * to learn anything from, which is scrollback with extra steps.
 *
 * Both words are the kernel's and are rendered verbatim. A kernel's vocabulary
 * for its own tools grows without warning (`docs/API_AGUI.md` §3), so an
 * unrecognised kind is drawn as it arrived rather than mapped onto a set this
 * client invented — and a session must never fail because a kernel named a
 * category nobody had heard of.
 */

interface ToolCallProps {
  readonly call: ToolCallEntry;
  /**
   * Open this call in the details column.
   *
   * Absent where there is no column to open — the row is then a summary and
   * says so by not being pressable, rather than by opening onto nothing.
   */
  readonly onInspect?: (call: ToolCallEntry) => void;
  /** Whether this is the call the column is currently showing. */
  readonly inspecting?: boolean;
}

/**
 * Statuses this surface treats as a failure.
 *
 * A short list rather than an exhaustive one, and everything else reads as
 * finished: §3 says these vocabularies differ per kernel and grow, so a status
 * nobody recognised must not be drawn as a failure on the strength of not
 * being recognised.
 */
const FAILED = new Set(['failed', 'error', 'denied', 'refused', 'cancelled', 'aborted']);

function toneOf(status: string | undefined): 'running' | 'failed' | 'done' {
  if (status === undefined) {
    return 'running';
  }
  return FAILED.has(status.toLowerCase()) ? 'failed' : 'done';
}

/**
 * The time a finished call took, formatted for the row — §18.2.
 *
 * Only when both instants are in the record: a running call has no finish, and
 * a finished call whose start or finish the log does not carry shows no
 * duration rather than one estimated.
 */
function durationOf(call: ToolCallEntry): string | undefined {
  if (call.startedAt === undefined || call.finishedAt === undefined) {
    return undefined;
  }
  const ms = Date.parse(call.finishedAt) - Date.parse(call.startedAt);
  if (!Number.isFinite(ms) || ms < 0) {
    return undefined;
  }
  if (ms < 1000) {
    return '<1s';
  }
  const totalSeconds = Math.round(ms / 1000);
  if (totalSeconds < 60) {
    return `${totalSeconds}s`;
  }
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${minutes}m ${seconds}s`;
}

/**
 * The bodies the log can carry, in the order it holds them.
 *
 * What a call was asked to do comes before what it produced, because that is
 * the order they happened in and the order a reader reconstructs them in.
 */
const BODIES = [
  { key: 'arguments', label: 'Arguments' },
  { key: 'result', label: 'Result' },
] as const;

/**
 * What this call has to show, in the order it happened.
 *
 * Built here rather than in the panel because it is a fact about the log's
 * shape, and the panel is a way of drawing things. A body that did not arrive
 * contributes nothing rather than an empty region — an absence is a fact, not
 * a zero.
 */
export function sectionsOf(call: ToolCallEntry): readonly DetailsSection[] {
  const sections: DetailsSection[] = [];
  for (const { key, label } of BODIES) {
    const body = call[key];
    if (body !== undefined) {
      sections.push({
        id: key,
        label,
        body: body.text,
        truncated: body.truncated,
        failed: key === 'result' && toneOf(call.status) === 'failed',
      });
    }
  }
  if (call.output !== undefined && call.output !== '') {
    sections.push({ id: 'output', label: 'Output', body: call.output });
  }
  return sections;
}

export function ToolCall({ call, onInspect, inspecting = false }: ToolCallProps) {
  const tone = toneOf(call.status);
  const duration = durationOf(call);
  const hasDetail = sectionsOf(call).length > 0;

  return (
    <div className="toolcall" data-tone={tone}>
      <button
        type="button"
        className="toolcall__row"
        /*
         * Not a disclosure any more. The row opens the details column rather
         * than growing: what a call carries is unbounded and is not prose,
         * and inside the reading column it pushed the conversation it belongs
         * to off the screen.
         */
        aria-pressed={inspecting}
        disabled={!hasDetail || onInspect === undefined}
        onClick={() => {
          onInspect?.(call);
        }}
      >
        {/*
         * The state as a dot, in the same vocabulary the session list uses.
         * A row that spelled its own state in a word did it in a face and a
         * case nothing else on the surface uses, and said it twice — once as
         * a glyph and once as RUNNING.
         */}
        <span className="toolcall__glyph">
          <StateDot state={tone === 'failed' ? 'error' : tone === 'running' ? 'ongoing' : 'done'} />
          <span className="sr-only">
            {tone === 'running' ? 'running' : (call.status ?? 'done')}
          </span>
        </span>

        <span className="toolcall__kind">{call.toolKind}</span>
        {call.target === undefined ? null : (
          <>
            <span className="toolcall__sep" aria-hidden="true" />
            <span className="toolcall__target">{call.target}</span>
          </>
        )}

        {duration === undefined ? null : <span className="toolcall__duration">{duration}</span>}
      </button>
    </div>
  );
}
