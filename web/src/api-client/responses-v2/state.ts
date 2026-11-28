import { ActivityState } from './activity-state.js';
import { ProtocolError } from '../errors.js';
import type { V2ResponseEvent } from './types.js';

/** Validate output references and text assembly, including a replay that starts mid-item. */
export class V2EventState {
  private readonly items = new Map<
    number,
    { id: string; text?: string; stage?: string; done?: boolean; activity?: ActivityState }
  >();
  private readonly ids = new Map<string, number>();
  private started = false;
  private progress = false;
  ended = false;
  constructor(
    private readonly responseId: string,
    private conversation: string | undefined,
    private readonly replay: boolean,
  ) {}

  accept(event: V2ResponseEvent) {
    if (this.ended) throw new ProtocolError('Responses frame follows the terminal event');
    if (event.type === 'error') throw new ProtocolError('Connection errors are not durable events');
    if (!this.started && !this.replay && event.type !== 'response.created')
      throw new ProtocolError('Responses must begin with response.created');
    this.started = true;
    if ('response' in event) {
      const response = event.response;
      const status =
        event.type === 'response.created' ? 'queued' : event.type.slice('response.'.length);
      if (
        response.id !== this.responseId ||
        !response.conversation.id ||
        (this.conversation !== undefined && response.conversation.id !== this.conversation) ||
        response.status !== status
      )
        throw new ProtocolError('Responses lifecycle identity or status is inconsistent');
      this.conversation ??= response.conversation.id;
      if (event.type === 'response.created') {
        if (event.sequence_number !== 0 || response.output.length)
          throw new ProtocolError('Invalid queued response');
      } else if (event.type === 'response.in_progress') {
        if (this.progress || response.output.length)
          throw new ProtocolError('Invalid in-progress response');
        this.progress = true;
      } else {
        this.ended = true;
        for (const [index, item] of this.items) {
          const output = response.output[index];
          if (item.activity) {
            if (
              output?.type !== 'mcp_call' ||
              output.id !== item.id ||
              !item.activity.matches(output)
            )
              throw new ProtocolError('Responses terminal activity disagrees with its events');
            continue;
          }
          if (
            output?.type !== 'message' ||
            !item.done ||
            output?.id !== item.id ||
            output.status !== 'completed' ||
            output.content.length !== 1 ||
            (item.text !== undefined && output.content[0]?.text !== item.text)
          )
            throw new ProtocolError('Responses terminal output disagrees with its events');
        }
        if (!this.replay && response.output.length !== this.items.size)
          throw new ProtocolError('Responses terminal output has unannounced items');
      }
      return;
    }
    if (!this.progress && !this.replay)
      throw new ProtocolError('Responses output preceded execution');
    const index = event.output_index;
    const id = 'item' in event ? event.item.id : event.item_id;
    if (!id || (this.ids.has(id) && this.ids.get(id) !== index))
      throw new ProtocolError('Responses item ID reused at another index');
    let item = this.items.get(index);
    if (item === undefined) {
      if (
        !this.replay &&
        (event.type !== 'response.output_item.added' || index !== this.items.size)
      )
        throw new ProtocolError('Responses output index has no added item');
      item = { id };
      this.items.set(index, item);
      this.ids.set(id, index);
    }
    if (item.id !== id || item.done)
      throw new ProtocolError('Responses item identity or lifecycle changed');
    const activity =
      ('item' in event && event.item.type === 'mcp_call') ||
      event.type.startsWith('response.mcp_call.');
    if (activity) {
      if (item.stage !== undefined) throw new ProtocolError('Responses message became activity');
      item.activity ??= new ActivityState(this.replay);
      item.activity.accept(event);
      return;
    }
    if (item.activity) throw new ProtocolError('Responses activity became a message');
    if ('content_index' in event && event.content_index !== 0)
      throw new ProtocolError('Responses text item has an invalid content index');
    const requireStage = (stage: string) => {
      if (item.stage !== stage && !(this.replay && item.stage === undefined))
        throw new ProtocolError('Responses content lifecycle is inconsistent');
    };
    switch (event.type) {
      case 'response.output_item.added':
        if (
          event.item.type !== 'message' ||
          item.stage !== undefined ||
          event.item.status !== 'in_progress' ||
          event.item.content.length
        )
          throw new ProtocolError('Responses item added more than once or with completed content');
        item.stage = 'added';
        break;
      case 'response.content_part.added':
        requireStage('added');
        if (event.part.text !== '')
          throw new ProtocolError('Responses content starts with nonempty text');
        item.text = '';
        item.stage = 'text';
        break;
      case 'response.output_text.delta':
        requireStage('text');
        if (item.text !== undefined) item.text += event.delta;
        item.stage = 'text';
        break;
      case 'response.output_text.done':
        requireStage('text');
        if (item.text !== undefined && item.text !== event.text)
          throw new ProtocolError('Responses text differs from its deltas');
        item.text = event.text;
        item.stage = 'text_done';
        break;
      case 'response.content_part.done':
        requireStage('text_done');
        if (item.text !== undefined && item.text !== event.part.text)
          throw new ProtocolError('Responses content differs from completed text');
        item.text = event.part.text;
        item.stage = 'part_done';
        break;
      case 'response.output_item.done':
        requireStage('part_done');
        if (
          event.item.type !== 'message' ||
          event.item.status !== 'completed' ||
          event.item.content.length !== 1 ||
          (item.text !== undefined && item.text !== event.item.content[0]?.text)
        )
          throw new ProtocolError('Responses completed item content is inconsistent');
        item.text = event.item.content[0]!.text;
        item.done = true;
    }
  }
}
