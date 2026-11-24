import { useId, useState } from 'react';
import { Button } from '../ui/Button';

export function PreparationMessage({ text }: { readonly text: string }) {
  const id = useId();
  const [expanded, setExpanded] = useState(false);
  const long = text.length > 1200 || text.split('\n').length > 6;
  return (
    <section className="preparation-message" aria-label="Original request">
      <p
        id={id}
        className="preparation-message__text"
        data-collapsed={(long && !expanded) || undefined}
      >
        {text}
      </p>
      {long ? (
        <Button
          variant="ghost"
          aria-expanded={expanded}
          aria-controls={id}
          onClick={() => setExpanded(!expanded)}
        >
          {expanded ? 'Show less' : 'Show more'}
        </Button>
      ) : null}
    </section>
  );
}
