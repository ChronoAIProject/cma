import { countTokens, setMergeCacheSize } from 'gpt-tokenizer/encoding/o200k_base';
import { visibleTokenCounter, type VisibleText } from './tokenEstimate';

setMergeCacheSize(512);
const count = visibleTokenCounter((text) => countTokens(text, { disallowedSpecial: new Set() }));
self.onmessage = (
  event: MessageEvent<{ readonly parts: readonly VisibleText[]; readonly basis: string }>,
) => {
  self.postMessage({ ...count(event.data.parts), basis: event.data.basis });
};
