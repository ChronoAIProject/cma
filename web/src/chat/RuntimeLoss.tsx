import type { RuntimeLoss as RuntimeLossState } from '@chronoai/cma-sdk';
import { fullTimestamp } from '../sessions/sessionTime';
import { Button } from '../ui/Button';

interface RuntimeLossProps {
  readonly loss?: RuntimeLossState;
  readonly onDismiss: () => void;
}

const RESTART_DETAIL =
  'A new runtime was started — the agent does not carry a conversation across a runtime it lost, so it begins again from what was captured.';

function headline(cause: string | undefined): string {
  switch (cause) {
    case 'out_of_memory':
      return "This session's runtime ran out of memory and was stopped.";
    case 'failed':
      return "This session's runtime stopped with an error.";
    case 'displaced':
      return 'The machine running this session was taken away.';
    case 'ended':
      return "This session's runtime exited.";
    case undefined:
      return "This session's runtime stopped. Nothing said why.";
    case 'unrecognised':
    default:
      return "This session's runtime stopped, for a reason this build does not recognise.";
  }
}

function detail(noticedAt: string | undefined): string {
  if (noticedAt === undefined) {
    return RESTART_DETAIL;
  }
  const parsed = Date.parse(noticedAt);
  return Number.isNaN(parsed)
    ? RESTART_DETAIL
    : `Noticed at ${fullTimestamp(parsed)}. ${RESTART_DETAIL}`;
}

export function RuntimeLoss({ loss, onDismiss }: RuntimeLossProps) {
  if (loss === undefined) {
    return null;
  }

  /*
   * History, not current failure. Reconciliation starts a replacement in the
   * same pass, so this takes the quiet notice and status semantics rather than
   * failure ink or an alert (`Web_Style.md` §16).
   */
  return (
    <div className="notice notice--quiet conversation__loss" role="status">
      <div>
        <p className="notice__title">{headline(loss.cause)}</p>
        <p className="notice__detail">{detail(loss.noticedAt)}</p>
      </div>
      <Button size="sm" onClick={onDismiss}>
        Dismiss
      </Button>
    </div>
  );
}
