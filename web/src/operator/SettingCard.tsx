import { useCallback, useState, type ReactNode } from 'react';
import type { StoredPlatformSettings } from '@cma/client';
import { Button } from '../ui/Button';
import { Timestamp } from './records';
import type { Setting } from './settingsInventory';

type Change = NonNullable<StoredPlatformSettings['setting_changes']>[string];

export function SettingsOverview({ inventory }: { readonly inventory: readonly Setting[] }) {
  return (
    <nav className="operator-settings-overview" aria-label="Settings overview">
      <h3>Settings overview</h3>
      <dl>
        {inventory.map(({ id, title, value }) => (
          <div key={id}>
            <dt>
              <a href={'#setting-' + id}>{title}</a>
            </dt>
            <dd>{value}</dd>
          </div>
        ))}
      </dl>
    </nav>
  );
}

function Values({ fields }: { readonly fields: NonNullable<Setting['fields']> }) {
  return (
    <dl className="operator-setting-values">
      {fields.map(([name, value]) => (
        <div key={name}>
          <dt>{name}</dt>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  );
}

export function SettingCard({
  setting,
  change,
  canReadAudit,
  disabled,
  children,
}: {
  readonly setting: Setting;
  readonly change?: Change;
  readonly canReadAudit: boolean;
  readonly disabled: boolean;
  readonly children?: ReactNode;
}) {
  const [editing, setEditing] = useState(false);
  const [showItems, setShowItems] = useState(false);
  const heading = 'setting-' + setting.id;
  const focusEditor = useCallback((editor: HTMLDivElement | null) => editor?.focus(), []);
  return (
    <section className="operator-setting-card" aria-labelledby={heading}>
      <header>
        <div>
          <h3 id={heading} tabIndex={-1}>
            {setting.title}
          </h3>
          <p className="operator-setting-value">{setting.value}</p>
        </div>
        {children != null && !editing && (
          <Button
            variant="outline"
            size="sm"
            disabled={disabled}
            aria-label={(setting.readOnly ? 'View fields for ' : 'Edit ') + setting.title}
            onClick={() => setEditing(true)}
          >
            {setting.readOnly ? 'View fields' : 'Edit'}
          </Button>
        )}
      </header>
      <p className="operator-notice">{setting.description}</p>
      {setting.fields && <Values fields={setting.fields} />}
      {!!setting.items?.length && (
        <details onToggle={(event) => setShowItems(event.currentTarget.open)}>
          <summary>Configured values</summary>
          {showItems && (
            <ul className="operator-setting-items">
              {setting.items.map(({ label, fields }) => (
                <li key={label}>
                  <strong>{label}</strong>
                  <Values fields={fields} />
                </li>
              ))}
            </ul>
          )}
        </details>
      )}
      {setting.details && (
        <details>
          <summary>Details</summary>
          <p className="operator-notice">{setting.details}</p>
        </details>
      )}
      <p className="operator-setting-audit">
        {!canReadAudit ? (
          'Last change requires activity-log permission.'
        ) : change ? (
          <>
            Last changed by {change.actor_name ?? change.actor_reference},{' '}
            <Timestamp value={change.at} /> (SGT)
          </>
        ) : (
          'Last change not recorded'
        )}
      </p>
      {setting.readOnly && <p className="operator-notice">{setting.readOnly}</p>}
      {editing && (
        <div className="operator-setting-editor" ref={focusEditor} tabIndex={-1}>
          {children}
        </div>
      )}
    </section>
  );
}
