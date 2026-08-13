import { describe, expect, it, vi } from 'vitest';
import { createInputGate } from './inputGate';
import type { TerminalDriver, TerminalSink } from '../terminal/types';

function recordingDriver() {
  const sent: string[] = [];
  const driver: TerminalDriver = {
    attach: vi.fn(),
    send: (data: string) => {
      sent.push(data);
    },
    resize: vi.fn(),
    detach: vi.fn(),
  };
  return { driver, sent };
}

describe('createInputGate', () => {
  it('starts closed', () => {
    const { driver, sent } = recordingDriver();
    const gate = createInputGate(driver);

    gate.driver.send('before anyone said this node had focus');

    expect(sent).toEqual([]);
  });

  it('forwards keystrokes once open', () => {
    const { driver, sent } = recordingDriver();
    const gate = createInputGate(driver);
    gate.setOpen(true);

    gate.driver.send('l');
    gate.driver.send('s');

    expect(sent).toEqual(['l', 's']);
  });

  it('drops keystrokes again once closed', () => {
    const { driver, sent } = recordingDriver();
    const gate = createInputGate(driver);
    gate.setOpen(true);
    gate.driver.send('a');

    gate.setOpen(false);
    gate.driver.send('delete everything\r');

    expect(sent).toEqual(['a']);
  });

  it('never replays what it dropped once reopened', () => {
    const { driver, sent } = recordingDriver();
    const gate = createInputGate(driver);

    gate.driver.send('typed at the wrong terminal');
    gate.setOpen(true);
    gate.driver.send('ok');

    // Buffering would deliver the earlier text to a session nobody was looking
    // at when they typed it.
    expect(sent).toEqual(['ok']);
  });

  it('tolerates being set to the state it is already in', () => {
    const { driver, sent } = recordingDriver();
    const gate = createInputGate(driver);
    gate.setOpen(true);
    gate.setOpen(true);

    gate.driver.send('a');

    expect(sent).toEqual(['a']);
  });

  it('keeps output, geometry and lifecycle flowing while closed', () => {
    const { driver } = recordingDriver();
    const gate = createInputGate(driver);
    const sink: TerminalSink = { write: vi.fn(), fail: vi.fn(), finish: vi.fn() };

    gate.driver.attach(sink);
    gate.driver.resize(120, 40);
    gate.driver.detach();

    // An unfocused node is still attached and still rendering; only its
    // keyboard path is closed.
    expect(driver.attach).toHaveBeenCalledWith(sink);
    expect(driver.resize).toHaveBeenCalledWith(120, 40);
    expect(driver.detach).toHaveBeenCalledOnce();
  });

  it('gates each node independently', () => {
    const a = recordingDriver();
    const b = recordingDriver();
    const gateA = createInputGate(a.driver);
    const gateB = createInputGate(b.driver);

    gateA.setOpen(true);
    gateA.driver.send('for-a');
    gateB.driver.send('for-a');

    gateA.setOpen(false);
    gateB.setOpen(true);
    gateA.driver.send('for-b');
    gateB.driver.send('for-b');

    expect(a.sent).toEqual(['for-a']);
    expect(b.sent).toEqual(['for-b']);
  });
});
