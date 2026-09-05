import { describe, expect, it, vi } from 'vitest';
import { ClientUnavailableError, CommandKeys, unavailableClient } from './client';

describe('web client boundary', () => {
  it('retains a command key through retries and rotates it after settlement', () => {
    const makeKey = vi.fn().mockReturnValueOnce('key-one').mockReturnValueOnce('key-two');
    const keys = new CommandKeys(makeKey);

    expect(keys.for('agent:agt_1:stop')).toBe('key-one');
    expect(keys.for('agent:agt_1:stop')).toBe('key-one');
    expect(makeKey).toHaveBeenCalledOnce();

    keys.settle('agent:agt_1:stop');
    expect(keys.for('agent:agt_1:stop')).toBe('key-two');
  });

  it('isolates simultaneous logical commands', () => {
    const keys = new CommandKeys(() => crypto.randomUUID());
    expect(keys.for('agent:one:stop')).not.toBe(keys.for('agent:two:stop'));
  });

  it('keeps the temporary production hierarchy empty and fails unsupported operations', async () => {
    await expect(unavailableClient.listSandboxes()).resolves.toEqual({ items: [] });
    await expect(unavailableClient.readAgent('agt_1')).rejects.toBeInstanceOf(
      ClientUnavailableError,
    );
  });
});
