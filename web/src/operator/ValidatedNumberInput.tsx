import { useId, useState, type ComponentProps } from 'react';
import { Input } from '../ui/Input';

/** Keep the accessible error aligned with the browser's numeric constraints. */
export function ValidatedNumberInput({
  error,
  ...props
}: Omit<ComponentProps<typeof Input>, 'type' | 'ref' | 'aria-invalid' | 'aria-describedby'> & {
  readonly error?: string;
}) {
  const id = useId();
  const [nativeError, setNativeError] = useState<string>();
  const message = error ?? nativeError;
  return (
    <>
      <Input
        {...props}
        type="number"
        ref={(input) => {
          // Recheck after controlled values or constraints change, including reset.
          if (input) setNativeError(input.validationMessage || undefined);
        }}
        aria-invalid={message !== undefined}
        aria-describedby={message === undefined ? undefined : id}
      />
      {message === undefined ? null : (
        <span id={id} className="operator-failure">
          {message}
        </span>
      )}
    </>
  );
}
