import { useId, useLayoutEffect, useRef, type RefObject } from 'react';
import { Button } from '../../ui/Button';
import { revealInvalidField } from './AdvancedFields';

const editable =
  'input:not(:disabled):not([readonly]), textarea:not(:disabled):not([readonly]), button:not(:disabled)';

function errorField(root: HTMLElement, error: string) {
  const pointer = /^(\/[^\s:]*):/.exec(error)?.[1];
  if (!pointer) return undefined;
  const candidates = [...root.querySelectorAll<HTMLElement>('[data-validation-path]')]
    .filter((element) => {
      const path = element.dataset.validationPath!;
      return pointer === path || pointer.startsWith(`${path}/`);
    })
    .sort((a, b) => b.dataset.validationPath!.length - a.dataset.validationPath!.length);
  for (const element of candidates) {
    const control = element.querySelector<HTMLElement>(editable);
    if (control) return control;
  }
}

export function ValidationError({
  error,
  canEdit,
  root,
}: {
  error: string;
  canEdit: boolean;
  root: RefObject<HTMLDivElement | null>;
}) {
  const summary = useRef<HTMLDivElement>(null);
  const id = useId();
  useLayoutEffect(() => {
    const field = canEdit && root.current ? errorField(root.current, error) : undefined;
    if (!field) {
      summary.current?.focus();
      return;
    }
    revealInvalidField(field);
    const previous = field.getAttribute('aria-describedby');
    field.setAttribute('aria-describedby', [previous, id].filter(Boolean).join(' '));
    field.focus();
    return () => {
      if (previous === null) field.removeAttribute('aria-describedby');
      else field.setAttribute('aria-describedby', previous);
    };
  }, [error, canEdit, root, id]);

  return (
    <div role="alert" tabIndex={-1} ref={summary}>
      <p id={id}>{error}</p>
      {canEdit ? (
        <Button
          onClick={() => {
            root.current?.querySelectorAll('details').forEach((details) => {
              details.open = true;
            });
            root.current?.querySelector<HTMLElement>(editable)?.focus();
          }}
        >
          Review channel fields
        </Button>
      ) : null}
    </div>
  );
}
