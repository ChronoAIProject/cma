import type { ReactNode } from 'react';

export function OperationDetails({
  number,
  title,
  status,
  output,
  children,
}: {
  readonly number: number;
  readonly title: string;
  readonly status: string;
  readonly output?: { readonly text: string; readonly truncated: boolean };
  readonly children?: ReactNode;
}) {
  return (
    <details className="operation-record">
      <summary>
        <span>Command {number}</span>
        <span className="operation-record__status">{status}</span>
      </summary>
      <div className="operation-record__details">
        <pre aria-label="Recorded command">{title}</pre>
        {output ? (
          <>
            <pre aria-label="Recorded output">{output.text}</pre>
            {output.truncated ? <p>Output truncated</p> : null}
          </>
        ) : (
          <p>Output not reported</p>
        )}
        {children}
      </div>
    </details>
  );
}
