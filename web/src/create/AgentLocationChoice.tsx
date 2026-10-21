import { useId, useRef, type KeyboardEvent } from 'react';
import { Alert } from '../ui/Alert';
import { StatusBadge } from '../ui/Badge';
import { Button } from '../ui/Button';
import { SkeletonRows } from '../ui/Skeleton';
import type { AgentLocation, AgentLocationOption } from './agentLocations';
import './AgentLocationChoice.css';

const STEP: Readonly<Record<string, number>> = {
  ArrowDown: 1,
  ArrowRight: 1,
  ArrowUp: -1,
  ArrowLeft: -1,
};

/**
 * "Where it works" — `docs/Web_Style.md` §12.2.
 *
 * Buttons with the radio role rather than native radios, so an option that
 * cannot be used now stays focusable as `aria-disabled` and its reason can be
 * read; a disabled native radio drops out of the keyboard order with it.
 */
export function AgentLocationChoice({
  options,
  value,
  onChange,
  locked,
  loading,
  failed,
  onRetry,
}: {
  readonly options: readonly AgentLocationOption[];
  readonly value: string | undefined;
  readonly onChange: (location: AgentLocation) => void;
  /** No location may change while its request is in flight. */
  readonly locked: boolean;
  readonly loading: boolean;
  readonly failed: boolean;
  readonly onRetry: () => void;
}) {
  const id = useId();
  const buttons = useRef(new Map<string, HTMLButtonElement>());
  const tabStop =
    options.find((option) => option.id === value)?.id ??
    options.find((option) => option.reason === undefined)?.id ??
    options[0]?.id;
  const choose = (option: AgentLocationOption) => {
    if (!locked && option.reason === undefined) onChange(option.location);
  };
  const onKeyDown = (event: KeyboardEvent, index: number) => {
    const target =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? options.length - 1
          : STEP[event.key] === undefined
            ? undefined
            : (index + STEP[event.key]! + options.length) % options.length;
    if (target === undefined) return;
    event.preventDefault();
    const option = options[target]!;
    buttons.current.get(option.id)?.focus();
    choose(option);
  };
  return (
    <div className="create-form__field">
      <span className="field__label" id={`${id}-label`}>
        Where it works
      </span>
      <div className="agent-location" role="radiogroup" aria-labelledby={`${id}-label`}>
        {options.map((option, index) => (
          <button
            key={option.id}
            ref={(element) => {
              if (element) buttons.current.set(option.id, element);
              else buttons.current.delete(option.id);
            }}
            type="button"
            role="radio"
            className="agent-location__option"
            aria-checked={option.id === value}
            aria-disabled={locked || option.reason !== undefined || undefined}
            aria-labelledby={`${id}-${index}-title`}
            aria-describedby={`${id}-${index}-detail`}
            tabIndex={option.id === tabStop ? 0 : -1}
            onClick={() => choose(option)}
            onKeyDown={(event) => onKeyDown(event, index)}
          >
            <span className="agent-location__title" id={`${id}-${index}-title`}>
              {option.title}
            </span>
            <span className="agent-location__detail" id={`${id}-${index}-detail`}>
              {option.status === undefined ? null : <StatusBadge status={option.status} />}
              {option.reason ?? option.detail}
            </span>
          </button>
        ))}
      </div>
      {loading ? <SkeletonRows rows={2} label="Loading your Sandboxes" /> : null}
      {failed ? (
        <Alert
          tone="warning"
          actions={
            <Button size="sm" onClick={onRetry}>
              Try again
            </Button>
          }
        >
          Your Sandboxes could not be loaded.
        </Alert>
      ) : null}
    </div>
  );
}
