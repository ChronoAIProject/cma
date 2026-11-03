const { refused } = require('./control.cjs');
const keys = { Backspace: 8, Tab: 9, Enter: 13, Escape: 27, Home: 36, End: 35,
  ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40, Delete: 46 };

async function sendInput(browser, session, input) {
  if (!input || typeof input !== 'object') throw refused('browser_input_invalid');
  if (input.kind === 'text' && typeof input.text === 'string' && input.text.length > 0 && input.text.length <= 1024) {
    await browser.call('Input.insertText', { text: input.text }, session);
  } else if (input.kind === 'key' && Object.hasOwn(keys, input.key)) {
    const key = { key: input.key, windowsVirtualKeyCode: keys[input.key], nativeVirtualKeyCode: keys[input.key] };
    await browser.call('Input.dispatchKeyEvent', { ...key, type: 'keyDown', ...(input.key === 'Enter' ? { text: '\r' } : {}) }, session);
    await browser.call('Input.dispatchKeyEvent', { ...key, type: 'keyUp' }, session);
  } else if (input.kind === 'click' && Number.isInteger(input.x) && Number.isInteger(input.y)
    && input.x >= 0 && input.x < 1280 && input.y >= 0 && input.y < 720) {
    const mouse = { x: input.x, y: input.y, button: 'left', clickCount: 1 };
    await browser.call('Input.dispatchMouseEvent', { ...mouse, type: 'mousePressed' }, session);
    await browser.call('Input.dispatchMouseEvent', { ...mouse, type: 'mouseReleased' }, session);
  } else throw refused('browser_input_invalid');
  return { applied: true };
}

module.exports = { sendInput };
