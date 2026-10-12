import type { KeyboardEvent, ReactNode } from 'react';
import { StateDot, type StateDotState } from '../ui/StateDot';
import { statusLabel } from '../vocabulary/status';

export function dotFor(status: string): StateDotState {
  if (status === 'failed' || status === 'deleted' || status === 'retired') return 'error';
  if (status === 'paused' || status === 'waiting_approval') return 'warning';
  if (
    [
      'working',
      'starting',
      'provisioning',
      'installing',
      'pausing',
      'reviving',
      'rotating',
      'credential_rotating',
      'retiring',
      'removing',
    ].includes(status)
  )
    return 'ongoing';
  return 'done';
}

export function stateMeta(status: Parameters<typeof statusLabel>[0], extra?: ReactNode) {
  return (
    <>
      <span className="tree-row__state">
        <StateDot state={dotFor(status)} size={8} />
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
