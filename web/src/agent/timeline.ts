import type {
  ApprovalRequest,
  ConversationMessage,
  PlanSnapshot,
  ReasoningBlock,
  RecoveryNotice,
  ToolCall,
} from './model';
import { toolOutcome } from './toolStatus';

export type Entry =
  | { kind: 'message'; item: ConversationMessage }
  | { kind: 'tool'; item: ToolCall }
  | { kind: 'reasoning'; item: ReasoningBlock }
  | { kind: 'approval'; item: ApprovalRequest }
  | { kind: 'plan'; item: PlanSnapshot }
  | { kind: 'recovery'; item: RecoveryNotice };

/** Two or more tool calls with nothing else recorded between them. */
export interface ToolGroup {
  readonly kind: 'tools';
  /** Keyed by its first call, which stays first while later calls join. */
  readonly id: string;
  readonly items: readonly ToolCall[];
  /** A group holding a failure opens itself, so the failure is never folded away. */
  readonly failed: boolean;
}

export type TimelineItem = Entry | ToolGroup;

/**
 * Fold each run of consecutive tool entries into one group. A lone call stays a
 * row of its own, because a disclosure around one row only adds a click.
 */
export function groupToolCalls(entries: readonly Entry[]): TimelineItem[] {
  const items: TimelineItem[] = [];
  let run: ToolCall[] = [];
  const flush = () => {
    const [first] = run;
    if (first === undefined) return;
    if (run.length === 1) items.push({ kind: 'tool', item: first });
    else
      items.push({
        kind: 'tools',
        id: `tools:${first.id}`,
        items: run,
        // Failure is a recorded status, so the active turn does not change it.
        failed: run.some((tool) => toolOutcome(tool, undefined) === 'failed'),
      });
    run = [];
  };
  for (const entry of entries) {
    if (entry.kind === 'tool') {
      run.push(entry.item);
      continue;
    }
    flush();
    items.push(entry);
  }
  flush();
  return items;
}
