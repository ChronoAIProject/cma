import { ProtocolError, TransportError } from '../errors.js';
import { MAX_AGENT_BYTES } from './schema.js';

/** Bound raw bytes before UTF-8 decoding and parser buffering can accumulate an incomplete frame. */
export class AgentFrameBound {
  private bytes = 0;
  private lineContent = false;
  private previousCr = false;
  private frameContent = false;
  inspect(chunk: Uint8Array) {
    for (const byte of chunk) {
      this.bytes++;
      if (this.bytes > MAX_AGENT_BYTES + 1024)
        throw new ProtocolError('Agent raw frame exceeds its byte boundary');
      if (byte === 10 && this.previousCr) {
        this.previousCr = false;
        continue;
      }
      this.previousCr = byte === 13;
      if (byte === 10 || byte === 13) {
        if (!this.lineContent) {
          this.bytes = 0;
          this.frameContent = false;
        }
        this.lineContent = false;
      } else {
        if (!this.lineContent && byte !== 58) this.frameContent = true;
        this.lineContent = true;
      }
    }
  }
  finish() {
    if (this.frameContent) throw new TransportError('Agent stream closed inside a frame');
  }
}
