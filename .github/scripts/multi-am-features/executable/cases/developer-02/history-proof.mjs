export function exportRows(transcript) {
 if(!transcript?.agent_id||!Array.isArray(transcript.turns))throw Error('transcript_shape');
 return transcript.turns.flatMap(t=>{
  const turn={agent_id:transcript.agent_id,through_seq:transcript.through_seq,turn_sequence:t.turn_sequence,started_at:t.started_at,outcome:t.outcome};
  return t.messages.length?t.messages.map(m=>({...turn,sequence:m.sequence,role:m.role,content:m.content,appended_at:m.appended_at})):[{...turn,messages:[]}];
 });
}
export function completeExport(raw,transcript){
 if(Buffer.byteLength(raw)>16*1024*1024)return false;
 try{const lines=raw.trim().split('\n').filter(Boolean).map(JSON.parse),expected=exportRows(transcript);
  return lines.length===expected.length&&lines.every((line,i)=>Object.keys(line).length===Object.keys(expected[i]).length
   &&Object.entries(expected[i]).every(([k,v])=>JSON.stringify(line[k])===JSON.stringify(v)));
 }catch{return false;}
}
export function retainedEntries(before,after){
 const all=after.turns.flatMap(t=>t.messages.map(m=>({turn:t.turn_sequence,...m})));
 const ids=all.map(m=>`${m.turn}:${m.sequence}`);
 return new Set(ids).size===ids.length&&before.turns.every(t=>t.messages.every(m=>all.some(n=>n.turn===t.turn_sequence&&n.sequence===m.sequence&&n.role===m.role&&n.content===m.content)));
}
