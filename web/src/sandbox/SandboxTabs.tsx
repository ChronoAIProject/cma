import { useId, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import type { SandboxTab } from './sandboxTabRoute';
import './SandboxTabs.css';

export interface SandboxTabPanel {
  readonly id: SandboxTab;
  readonly label: string;
  readonly content: ReactNode;
  /**
   * Render only after the tab is first selected.
   *
   * For a panel whose content starts its own reads, so a tab nobody opens
   * costs nothing; once opened it stays mounted like the others.
   */
  readonly lazy?: boolean;
}

/**
 * The Sandbox page's tablist and panels — `docs/Web_Style.md` §17.
 *
 * Every panel stays mounted while hidden, so a scope selection that has not
 * been saved, or a connection check in flight, survives a look at another
 * tab. Arrow keys move and select (automatic activation: switching a panel
 * costs nothing here), Home and End go to the ends, and only the selected tab
 * is in the tab order.
 */
export function SandboxTabs({
  label,
  tabs,
  selected,
  onSelect,
}: {
  readonly label: string;
  readonly tabs: readonly SandboxTabPanel[];
  readonly selected: SandboxTab;
  readonly onSelect: (tab: SandboxTab) => void;
}) {
  const id = useId();
  const buttons = useRef(new Map<SandboxTab, HTMLButtonElement>());
  const [visited, setVisited] = useState<ReadonlySet<SandboxTab>>(() => new Set([selected]));
  // A selection can also arrive from outside, such as View progress.
  if (!visited.has(selected)) setVisited(new Set([...visited, selected]));
  const move = (event: KeyboardEvent, index: number) => {
    const next =
      event.key === 'ArrowRight'
        ? (index + 1) % tabs.length
        : event.key === 'ArrowLeft'
          ? (index + tabs.length - 1) % tabs.length
          : event.key === 'Home'
            ? 0
            : event.key === 'End'
              ? tabs.length - 1
              : undefined;
    const target = next === undefined ? undefined : tabs[next];
    if (target === undefined) return;
    event.preventDefault();
    onSelect(target.id);
    buttons.current.get(target.id)?.focus();
  };
  return (
    <div className="sandbox-tabs">
      <div className="sandbox-tabs__list" role="tablist" aria-label={label}>
        {tabs.map((tab, index) => (
          <button
            key={tab.id}
            ref={(element) => {
              if (element === null) buttons.current.delete(tab.id);
              else buttons.current.set(tab.id, element);
            }}
            id={`${id}-tab-${tab.id}`}
            type="button"
            role="tab"
            aria-selected={tab.id === selected}
            aria-controls={`${id}-panel-${tab.id}`}
            tabIndex={tab.id === selected ? 0 : -1}
            onClick={() => onSelect(tab.id)}
            onKeyDown={(event) => move(event, index)}
          >
            {tab.label}
          </button>
        ))}
      </div>
      {tabs.map((tab) => (
        <div
          key={tab.id}
          className="sandbox-tabs__panel"
          id={`${id}-panel-${tab.id}`}
          role="tabpanel"
          aria-labelledby={`${id}-tab-${tab.id}`}
          hidden={tab.id !== selected}
        >
          {tab.lazy === true && !visited.has(tab.id) ? null : tab.content}
        </div>
      ))}
    </div>
  );
}
