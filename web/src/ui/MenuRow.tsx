import { useId } from 'react';
import { IconCheckOutline16 } from './icons';
import type { MenuEntry } from './Menu';

export function MenuRow({
  entry,
  selected,
  choosing,
  onSelect,
}: {
  readonly entry: MenuEntry;
  readonly selected: boolean;
  /** Whether this menu is picking a value rather than running an action. */
  readonly choosing: boolean;
  readonly onSelect: (id: string) => void;
}) {
  const reasonId = useId();
  if ('type' in entry && entry.type === 'separator') {
    return <div className="menu__separator" role="separator" />;
  }
  if ('type' in entry && entry.type === 'label') {
    return (
      <div className="menu__label" role="presentation">
        {entry.text}
      </div>
    );
  }
  if ('type' in entry && entry.type === 'choices') {
    return (
      <div className="menu__choice-group" role="group" aria-label={entry.text}>
        <div className="menu__label" aria-hidden="true">
          {entry.text}
        </div>
        <div className="menu__choices">
          {entry.choices.map((choice) => (
            <button
              key={choice.id}
              type="button"
              role="menuitemradio"
              className="menu__item menu__choice"
              data-id={choice.id}
              aria-checked={entry.selectedId === choice.id}
              onClick={() => onSelect(choice.id)}
            >
              {choice.label}
            </button>
          ))}
        </div>
      </div>
    );
  }
  if ('type' in entry) {
    return null;
  }

  const unavailable = entry.disabled === true;
  const content = (
    <>
      {entry.icon === undefined ? null : (
        <span className="menu__icon" aria-hidden="true">
          {entry.icon}
        </span>
      )}
      <span className="menu__itemlabel">
        {entry.reason === undefined ? (
          entry.label
        ) : (
          <>
            <span className="menu__labeltext">{entry.label}</span>{' '}
            <span className="menu__reason" id={reasonId}>
              {entry.reason}
            </span>
          </>
        )}
      </span>
      {selected ? <IconCheckOutline16 className="menu__check" /> : null}
    </>
  );
  const shared = {
    /*
     * A menu with a current value is a set of alternatives, not a list of
     * actions, and `menuitemradio` is what says so — it is also the role
     * `aria-checked` is actually defined for.
     */
    role: entry.role ?? (choosing ? 'menuitemradio' : 'menuitem'),
    'data-id': entry.id,
    'aria-label': entry.role && typeof entry.label === 'string' ? entry.label : undefined,
    'aria-describedby': entry.role && entry.reason ? reasonId : undefined,
    'data-has-reason': entry.reason === undefined ? undefined : '',
    className: ['menu__item', entry.danger === true ? 'menu__item--danger' : '']
      .filter(Boolean)
      .join(' '),
    /*
     * `aria-disabled` rather than `disabled`, so the row keeps its place in
     * the keyboard order. A row that cannot be reached cannot have its
     * reason read, and the reason is the whole point of listing something
     * that cannot be chosen (§12.3).
     */
    'aria-disabled': unavailable,
    'aria-checked': choosing || entry.role ? selected : undefined,
  } as const;
  if (entry.href !== undefined && !unavailable) {
    return (
      <a
        {...shared}
        href={entry.href}
        onClick={(event) => {
          // A modified click is the browser's: a new tab or window, not a
          // selection in this one.
          if (
            event.button !== 0 ||
            event.metaKey ||
            event.ctrlKey ||
            event.shiftKey ||
            event.altKey
          ) {
            return;
          }
          event.preventDefault();
          onSelect(entry.id);
        }}
      >
        {content}
      </a>
    );
  }
  return (
    <button
      type="button"
      {...shared}
      onClick={() => {
        if (unavailable) {
          return;
        }
        onSelect(entry.id);
      }}
    >
      {content}
    </button>
  );
}
