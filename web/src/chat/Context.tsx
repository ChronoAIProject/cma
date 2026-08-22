import type { Usage as UsageState } from './conversation';
import { formatTokens } from './Usage';
import './Context.css';

/** How full the conversation's context is against its stated ceiling. */
interface ContextProps {
  /** Whether the session was granted usage reporting at all. */
  readonly granted: boolean;
  /** The latest readings, where the runtime has reported them. */
  readonly usage?: UsageState;
}

export function Context({ granted, usage }: ContextProps) {
  if (!granted) {
    return null;
  }

  const used = usage?.contextUsed;
  if (used === undefined || used <= 0) {
    return <span className="context">Context · waiting</span>;
  }

  const window = usage?.contextWindow;
  if (window === undefined || window <= 0) {
    return <span className="context">{formatTokens(used)} in context</span>;
  }

  const percent = Math.min(100, (used / window) * 100);
  return (
    <div className="context">
      <span>
        {formatTokens(used)} / {formatTokens(window)} context
      </span>
      <span
        className="context__track"
        role="progressbar"
        aria-label="Conversation context used"
        aria-valuemin={0}
        aria-valuenow={used}
        aria-valuemax={window}
      >
        <span className="context__fill" style={{ width: `${String(percent)}%` }} />
      </span>
    </div>
  );
}
