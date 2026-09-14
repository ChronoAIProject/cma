import type { ProfileKind } from '../routing/routes';

export const PROFILE_KINDS: readonly { readonly kind: ProfileKind; readonly label: string }[] = [
  { kind: 'sandbox', label: 'Sandbox' },
  { kind: 'workspace', label: 'Workspace' },
  { kind: 'agent', label: 'Agent' },
];

export function ProfileTabs({
  kind,
  onChange,
}: {
  readonly kind: ProfileKind;
  readonly onChange: (kind: ProfileKind) => void;
}) {
  return (
    <div className="profile-tabs" role="tablist" aria-label="Profile kind">
      {PROFILE_KINDS.map((entry, index) => (
        <button
          type="button"
          key={entry.kind}
          role="tab"
          id={`profile-tab-${entry.kind}`}
          aria-controls="profile-list"
          aria-selected={entry.kind === kind}
          tabIndex={entry.kind === kind ? 0 : -1}
          onClick={() => onChange(entry.kind)}
          onKeyDown={(event) => {
            const next =
              event.key === 'ArrowRight'
                ? (index + 1) % PROFILE_KINDS.length
                : event.key === 'ArrowLeft'
                  ? (index + PROFILE_KINDS.length - 1) % PROFILE_KINDS.length
                  : event.key === 'Home'
                    ? 0
                    : event.key === 'End'
                      ? PROFILE_KINDS.length - 1
                      : undefined;
            if (next === undefined) return;
            event.preventDefault();
            const target = PROFILE_KINDS[next];
            if (target !== undefined) {
              onChange(target.kind);
              document.getElementById(`profile-tab-${target.kind}`)?.focus();
            }
          }}
        >
          {entry.label}
        </button>
      ))}
    </div>
  );
}
