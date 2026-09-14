import type { ReactNode } from 'react';

export function Field({
  id,
  label,
  help,
  error,
  children,
}: {
  readonly id: string;
  readonly label: string;
  readonly help: string;
  readonly error?: string;
  readonly children: ReactNode;
}) {
  return (
    <div className="profile-field">
      <label htmlFor={id}>{label}</label>
      {children}
      <p id={`${id}-help`} className="profile-field__help">
        {help}
      </p>
      {error === undefined ? null : (
        <p id={`${id}-error`} className="profile-field__error">
          {error}
        </p>
      )}
    </div>
  );
}

export function fieldDescription(id: string, error?: string): string {
  return error === undefined ? `${id}-help` : `${id}-help ${id}-error`;
}
