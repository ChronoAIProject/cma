import './ViewSwitch.css';

/**
 * The view switch — docs/Web_Style.md §1 and §6.
 *
 * The frontend has exactly two views, and this is how somebody moves between
 * them. It is not navigation and must never become it: there is no route
 * behind a segment, nothing is pushed onto history, and a segment is offered
 * only when the control plane says the caller holds a capability the view can
 * act on (§14).
 *
 * A single-segment switch is drawn as one segment rather than hidden. The
 * caller who holds nothing beyond the everyday set sees where they are, which
 * is the same thing the caller who holds two sees.
 */

export interface ViewOption<Name extends string> {
  readonly name: Name;
  readonly label: string;
}

interface ViewSwitchProps<Name extends string> {
  readonly views: readonly ViewOption<Name>[];
  readonly active: Name;
  readonly onSelect: (name: Name) => void;
}

export function ViewSwitch<Name extends string>({
  views,
  active,
  onSelect,
}: ViewSwitchProps<Name>) {
  return (
    <div className="viewswitch" role="group" aria-label="View">
      {views.map((view) => (
        <button
          key={view.name}
          type="button"
          className="viewswitch__segment"
          // The selected segment is marked by the accent AND by pressed state,
          // so it does not depend on colour alone (§10).
          aria-pressed={view.name === active}
          onClick={() => {
            onSelect(view.name);
          }}
        >
          {view.label}
        </button>
      ))}
    </div>
  );
}
