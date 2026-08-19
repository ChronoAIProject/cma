import { useId, useState } from 'react';
import type { ToolCallEntry } from './conversation';
import './ToolCall.css';

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

export function ToolCall({ call }: ToolCallProps) {
  const [open, setOpen] = useState(false);
  const bodyId = useId();
  const tone = toneOf(call.status);
  const duration = durationOf(call);
  const hasDetail =
    (call.output !== undefined && call.output !== '') ||
    call.arguments !== undefined ||
    call.result !== undefined;

  return (
    <div className="toolcall" data-tone={tone}>
      <button
        type="button"
        className="toolcall__row"
        aria-expanded={open}
        aria-controls={hasDetail ? bodyId : undefined}
        disabled={!hasDetail}
        onClick={() => {
          setOpen((wasOpen) => !wasOpen);
        }}
      >
        {/*
         * The status glyph — §18.2: a tick for finished, a cross for failed,
         * a pulsing dot while it runs. Reinforced by the word at the row's
         * end, because §2 forbids colour carrying meaning alone.
         */}
        {tone === 'running' ? (
          <span className="toolcall__glyph toolcall__glyph--running pulse" aria-hidden="true" />
        ) : (
          <span className="toolcall__glyph" aria-hidden="true">
            {tone === 'failed' ? '×' : '✓'}
          </span>
        )}

        <span className="toolcall__kind">{call.toolKind}</span>
        {call.target === undefined ? null : <span className="toolcall__target">{call.target}</span>}

        {duration === undefined ? null : <span className="toolcall__duration">{duration}</span>}

        {/*
         * The status word, drawn for a failed or a running call. A finished
         * call's word is its glyph and its stillness; §18.2 spends the words
         * where the state needs one.
         */}
        {tone === 'done' && call.status === undefined ? null : (
          <span className="toolcall__status">{tone === 'running' ? 'running' : call.status}</span>
        )}
      </button>

      {/*
       * Disclosure, not truncation (§18.2). Output is agent-emitted content
       * and §1's fidelity rule applies, so nothing is silently shortened —
       * bounding it behind this control is a frame decision and is permitted,
       * dropping the tail of it is not. It scrolls inside itself.
       */}
      {open && hasDetail ? (
        <pre className="toolcall__output" id={bodyId}>
          {call.arguments === undefined
            ? null
            : `arguments${call.arguments.truncated ? ' (truncated)' : ''}:\n${call.arguments.text}\n`}
          {call.result === undefined
            ? null
            : `result${call.result.truncated ? ' (truncated)' : ''}:\n${call.result.text}\n`}
          {call.output ?? ''}
        </pre>
      ) : null}
    </div>
  );
}
