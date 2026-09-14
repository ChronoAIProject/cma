import { publicCopy } from '../vocabulary/needs';

export function WhatHappensNext({ sentences }: { readonly sentences: readonly string[] }) {
  return (
    <section className="public-section">
      <h2>{publicCopy.next}</h2>
      <ol>
        {sentences.map((sentence, index) => (
          <li key={index}>{sentence}</li>
        ))}
      </ol>
    </section>
  );
}
