import { BouncingText } from './BouncingText';
import { stageLabel, stageProgress, type Stage } from './progress';
import './StartupProgress.css';

/**
 * What is happening while a session starts.
 *
 * The wave says the page is alive; the bar says how far along the work is. They
 * answer different questions, which is why both are here — a wave alone cannot
 * distinguish a slow start from a stuck one, and a bar alone looks frozen
 * between steps.
 *
 * The bar reflects steps the control plane actually reported. It never advances
 * on a timer: a bar that moves without anything happening is a lie that costs
 * the user their trust in every other bar.
 */

interface StartupProgressProps {
  readonly message: string;
  /** The last step reported, or null if none has arrived yet. */
  readonly stage: Stage | null;
}

/** The shared, wire-truthful stage line, also used while a paused Chat revives. */
export function StartupStage({ stage }: Pick<StartupProgressProps, 'stage'>) {
  return (
    <p className="startup__stage" aria-live="polite">
      {stage === null ? 'Starting…' : stageLabel(stage)}
    </p>
  );
}

export function StartupProgress({ message, stage }: StartupProgressProps) {
  const fraction = stage === null ? 0 : stageProgress(stage);
  const percent = Math.round(fraction * 100);

  return (
    <div className="startup">
      <p className="startup__message">
        <BouncingText text={message} />
      </p>

      <div
        className="startup__track"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        aria-label="Session startup"
      >
        <div className="startup__fill" style={{ width: `${String(percent)}%` }} />
      </div>

      {/*
       * Named rather than left as a bare bar. "Asking for a sandbox" and
       * "connecting to the terminal" fail for different reasons, and knowing
       * which one is slow is most of what someone wants when it is slow.
       */}
      <StartupStage stage={stage} />
    </div>
  );
}
