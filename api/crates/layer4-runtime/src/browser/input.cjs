const { refused } = require('./control.cjs');

async function sendInput(browser, session, input) {
  if (!input || typeof input !== 'object') throw refused('browser_input_invalid');
  if (input.kind === 'text' && typeof input.text === 'string' && input.text.length > 0 && input.text.length <= 1024) {
    await browser.call('Input.insertText', { text: input.text }, session);
  } else if (input.kind === 'click' && Number.isInteger(input.x) && Number.isInteger(input.y)
    && input.x >= 0 && input.x < 1280 && input.y >= 0 && input.y < 720) {
    const mouse = { x: input.x, y: input.y, button: 'left', clickCount: 1 };
    await browser.call('Input.dispatchMouseEvent', { ...mouse, type: 'mousePressed' }, session);
    await browser.call('Input.dispatchMouseEvent', { ...mouse, type: 'mouseReleased' }, session);
  } else throw refused('browser_input_invalid');
  return { applied: true };
}

module.exports = { sendInput };
