import type { KeyboardEvent, ReactNode } from 'react';
import { StateDot } from '../ui/StateDot';
import { statusLabel } from '../vocabulary/status';

export function stateMeta(
  status: Parameters<typeof statusLabel>[0],
  extra?: ReactNode,
  resource: 'agent' | 'sandbox' | 'workspace' = 'agent',
) {
  return (
    <>
      <span className="tree-row__state">
        {(resource === 'agent' && status === 'working') ||
        (resource === 'sandbox' && status === 'active') ? (
          <StateDot state="done" size={8} />
        ) : null}
        {statusLabel(status)}
      </span>
      {extra}
    </>
  );
}

function visibleRows(tree: HTMLElement): HTMLElement[] {
  return [...tree.querySelectorAll<HTMLElement>('[role="treeitem"]')];
}

export function keyboardNavigation(event: KeyboardEvent<HTMLDivElement>) {
  if (!(event.target instanceof HTMLElement) || event.target.getAttribute('role') !== 'treeitem')
    return;
  const row = event.target;
  const tree = event.currentTarget;
  const rows = visibleRows(tree);
  const at = rows.indexOf(row);
  const move = (index: number) => {
    event.preventDefault();
    rows[index]?.focus();
  };
  if (event.key === 'ArrowDown') move(at + 1);
  else if (event.key === 'ArrowUp') move(at - 1);
  else if (event.key === 'Home') move(0);
  else if (event.key === 'End') move(rows.length - 1);
  else if (event.key === 'ArrowRight') {
    const disclosure = row.querySelector<HTMLButtonElement>('.tree-row__disclosure');
    if (row.getAttribute('aria-expanded') === 'false') disclosure?.click();
    else rows.find((candidate) => candidate.dataset.parentId === row.dataset.nodeId)?.focus();
    event.preventDefault();
  } else if (event.key === 'ArrowLeft') {
    if (row.getAttribute('aria-expanded') === 'true') {
      row.querySelector<HTMLButtonElement>('.tree-row__disclosure')?.click();
    } else {
      rows.find((candidate) => candidate.dataset.nodeId === row.dataset.parentId)?.focus();
    }
    event.preventDefault();
  } else if (event.key === 'Enter' || event.key === ' ') {
    row.querySelector<HTMLElement>('.tree-row__label')?.click();
    event.preventDefault();
  }
}

export function activityLabel(value: string, now = Date.now()): string {
  const elapsed = Math.max(0, now - Date.parse(value));
  if (!Number.isFinite(elapsed)) return 'Unknown activity';
  if (elapsed < 60_000) return 'Now';
  if (elapsed < 3_600_000) return `${String(Math.floor(elapsed / 60_000))}m`;
  if (elapsed < 86_400_000) return `${String(Math.floor(elapsed / 3_600_000))}h`;
  return `${String(Math.floor(elapsed / 86_400_000))}d`;
}
