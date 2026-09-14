import type { TriggerButtonLabel, TriggerTheme } from '@chronoai/cma-sdk';
import { LABELS } from './model';

const fixtures = import.meta.glob('./badges/*-available.svg', {
  eager: true,
  query: '?url',
  import: 'default',
}) as Record<string, string>;

/** Draft previews use the actual server golden assets, not a second badge renderer. */
export function DraftBadge({
  label,
  theme,
}: {
  readonly label: TriggerButtonLabel;
  readonly theme: TriggerTheme;
}) {
  return (
    <img
      height="28"
      src={fixtures[`./badges/${label}-${theme}-available.svg`]}
      alt={LABELS[label]}
    />
  );
}

export function BadgePreview({ url, label }: { readonly url: string; readonly label: string }) {
  const variant = (theme: 'light' | 'dark') => {
    const value = new URL(url);
    value.searchParams.set('theme', theme);
    return value.toString();
  };
  return (
    <div className="trigger-badge-previews">
      {(['light', 'dark'] as const).map((theme) => (
        <figure key={theme} className={`trigger-badge-preview trigger-badge-preview--${theme}`}>
          <img height="28" src={variant(theme)} alt={label} referrerPolicy="no-referrer" />
          <figcaption>{theme === 'light' ? 'Light' : 'Dark'}</figcaption>
        </figure>
      ))}
    </div>
  );
}
