import { useEffect, useState } from 'react';
import './CopyKey.css';

/**
 * Take something away from the surface — `Web_Style.md` §18.11.
 *
 * One control, used by a code block's header and by a turn's own action, so
 * the two cannot drift into different words or different guarantees.
 *
 * The pattern is §12's and is not reinvented here: the write is guarded,
 * because clipboard access needs a secure context and can be refused; the
 * control claims "Copied" only *after* the write resolves, because claiming it
 * before would report something that did not happen; and a refusal is
 * swallowed in silence, because the text is still selectable and an error
 * notice for something the reader can simply do by hand is noise.
 */

/** How long the control keeps saying it worked. */
const SAID_FOR_MS = 1600;

interface CopyKeyProps {
  /** What lands on the clipboard. The source, never the rendered tree. */
  readonly text: string;
  /** What the control says at rest. */
  readonly label?: string;
  /**
   * Why it cannot be used, when it cannot.
   *
   * Disabled and stated rather than removed — §7 and §19. A turn still being
   * written is the case this exists for: copying half an answer is the thing
   * to prevent, and taking the control away is the wrong instrument for it.
   */
  readonly refusedReason?: string;
}

export function CopyKey({ text, label = 'Copy', refusedReason }: CopyKeyProps) {
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) {
      return;
    }
    const timer = setTimeout(() => {
      setCopied(false);
    }, SAID_FOR_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [copied]);

  return (
    <button
      className="copykey"
      type="button"
      disabled={refusedReason !== undefined}
      title={refusedReason}
      aria-label={copied ? 'Copied' : label}
      onClick={() => {
        void navigator.clipboard
          ?.writeText(text)
          .then(() => {
            setCopied(true);
          })
          .catch(() => {
            // Refused by the browser. The text is still selectable.
          });
      }}
    >
      {copied ? 'Copied' : label}
    </button>
  );
}
