import type { KeyboardEvent, ReactNode } from 'react';
import { toneDot } from '../ui/Badge';
import { statusLabel, statusTone } from '../vocabulary/status';
import { CharacterAvatar, characterPhase } from '../character/CharacterAvatar';
import { characterForStatus } from '../character/status';
import type { StatusTone } from '../vocabulary/status';

interface ObservedPresentation {
  readonly label: string;
  readonly tone: StatusTone;
}

/** A row's status word with its dot from the shared tone (§12.1), then any facts. */
export function stateMeta(
  status: Parameters<typeof statusLabel>[0],
  extra?: ReactNode,
  observation?: ObservedPresentation,
) {
  const tone = observation?.tone ?? statusTone(status);
  return (
    <>
      <span className="tree-row__state" data-tone={tone}>
        {toneDot(tone)}
        {observation?.label ?? statusLabel(status)}
      </span>
      {extra}
    </>
  );
}

/**
 * An Agent row's leading slot (§10, §28): the Agent's character, posed by its
 * status and wearing its Sandbox's colour. Only work in progress or a person
 * being asked moves, so the rows that move are worth looking at. The word
 * stays in the hover title and the row's accessible name; the character itself
 * is decorative.
 */
export function agentSignal(
  status: Parameters<typeof statusLabel>[0],
  agentId: string,
  size: number,
  color?: string,
  observation?: ObservedPresentation,
) {
  const presence = characterForStatus(status);
  return (
    <span className="tree-row__signal" title={observation?.label ?? statusLabel(status)}>
      <CharacterAvatar
        pose={presence.pose}
        tone={observation?.tone === 'error' ? 'error' : presence.tone}
        color={color}
        still={presence.pose === 'preparing' || observation !== undefined || !presence.animate}
        phase={characterPhase(agentId)}
        size={size}
      />
    </span>
  );
}

/**
 * The one-line row's facts. The word is never drawn but stays in the row's
 * accessible name, so a screen reader hears every state the list used to show.
 * Its own separator keeps it from running into the title wherever layout does
 * not already separate the two.
 */
export function lineMeta(status: Parameters<typeof statusLabel>[0], updatedAt: string) {
  return (
    <>
      <span className="sr-only">{`, ${statusLabel(status)}`}</span>
      <time className="tree-row__age" dateTime={updatedAt}>
        {activityLabel(updatedAt)}
      </time>
    </>
  );
}

function visibleRows(tree: HTMLElement): HTMLElement[] {
  return [...tree.querySelectorAll<HTMLElement>('[role="treeitem"]')].filter(
    (row) => !row.closest('details:not([open]), [hidden]'),
  );
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
