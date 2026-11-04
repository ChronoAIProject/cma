import { useId } from 'react';

export function MachineOption({
  enabled,
  available,
  frozen,
  onChange,
}: {
  readonly enabled: boolean;
  readonly available: boolean;
  readonly frozen: boolean;
  readonly onChange: (enabled: boolean) => void;
}) {
  const description = useId();
  return (
    <div>
      <label className="task-home__desktop">
        <input
          type="checkbox"
          checked={enabled}
          disabled={frozen || (!enabled && !available)}
          aria-describedby={description}
          onChange={(event) => onChange(event.target.checked)}
        />
        Include a shared desktop
      </label>
      <p id={description}>
        {frozen
          ? 'Shared desktop choice is saved with the submitted task.'
          : available
            ? 'Adds a Linux desktop with the Chromium browser that you and your Agent can both see and control. Useful for work on websites.'
            : 'A shared desktop can only be added with permission to create a new Sandbox; an existing Sandbox keeps its original setup.'}
      </p>
    </div>
  );
}
