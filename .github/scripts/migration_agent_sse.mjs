import {identity,requireValue} from './migration_agent_observations.mjs';

// Incremental framing is byte-bounded even when a peer never sends a newline.
// Unknown event kinds are retained by the application reader, not interpreted
// here; every event carrying response identity must agree with admission.
export async function consumeResponses(body,agentId,created,{maxBytes=8*1024*1024,maxFrame=1024*1024}={}) {
  requireValue(body && identity(agentId) && typeof created === 'function', 'sse_input_invalid');
  const decoder=new TextDecoder('utf-8',{fatal:true});
  let bytes=0,buffer='',responseId=null,lastSequence=-1,terminal=false,done=false;
  function frame(text) {
    const data=text.split('\n').filter(line=>line.startsWith('data:'))
      .map(line=>line.slice(5).trimStart()).join('\n');
    if (!data) return;
    requireValue(!done,'sse_after_done');
    if (data==='[DONE]') {requireValue(terminal,'sse_done_before_terminal');done=true;return;}
    const event=JSON.parse(data);
    requireValue(event && typeof event.type==='string' && Number.isSafeInteger(event.sequence_number)
      && event.sequence_number>lastSequence, 'sse_sequence_invalid');
    lastSequence=event.sequence_number;
    if (event.type==='response.created') {
      requireValue(!responseId && event.sequence_number===0 && identity(event.response?.id)
        && event.response.conversation?.id===agentId, 'sse_admission_identity_invalid');
      responseId=event.response.id; created(responseId);
    } else requireValue(responseId,'sse_event_before_admission');
    if (event.response) requireValue(event.response.id===responseId
      && event.response.conversation?.id===agentId, 'sse_response_identity_changed');
    if (event.response_id!==undefined) requireValue(event.response_id===responseId,'sse_response_identity_changed');
    requireValue(!terminal,'sse_event_after_terminal');
    if (['response.completed','response.failed','response.incomplete'].includes(event.type)) {
      requireValue(event.response?.status===event.type.slice(9), 'sse_terminal_status_mismatch');
      requireValue(event.type==='response.completed','sse_terminal_failed');
      terminal=true;
    }
  }
  function drain() {
    let end;
    while ((end=buffer.indexOf('\n\n'))>=0) {
      requireValue(Buffer.byteLength(buffer.slice(0,end))<=maxFrame,'sse_frame_limit');
      const next=buffer.slice(0,end);buffer=buffer.slice(end+2);frame(next);
    }
    requireValue(Buffer.byteLength(buffer)<=maxFrame,'sse_frame_limit');
  }
  for await (const chunk of body) {
    bytes+=chunk.byteLength;requireValue(bytes<=maxBytes,'sse_byte_limit');
    buffer+=decoder.decode(chunk,{stream:true});buffer=buffer.replace(/\r\n/g,'\n');drain();
  }
  buffer+=decoder.decode();buffer=buffer.replace(/\r\n/g,'\n');drain();
  requireValue(buffer.trim()==='' && responseId && terminal,'sse_incomplete');
  return {response_id:responseId,terminal:'completed',bytes};
}
