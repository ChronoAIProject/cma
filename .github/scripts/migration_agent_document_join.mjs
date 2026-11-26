// Bind protected Mongo observations to the actual retained Agent journal.
import {constants} from 'node:fs';
import {open,realpath} from 'node:fs/promises';
import {join,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';
import {createHash} from 'node:crypto';
import {artifact,hash,identity,requireValue,same} from './migration_agent_observations.mjs';

const sha=value=>createHash('sha256').update(value).digest('hex');
const checksum=value=>typeof value==='string' && /^[a-f0-9]{64}$/.test(value);
const STAGES=['old-write','new-read-old-write','new-write','old-read-new-write'];
const COLLECTIONS=['agent_lifecycle_operations','agents','sandboxes','workspaces','worktrees'];

async function privateBytes(directory,name,max,prefix) {
  requireValue(await realpath(directory)===directory,'document_join_directory_invalid');
  const dir=await open(directory,constants.O_RDONLY|constants.O_NOFOLLOW);
  try {
    const stat=await dir.stat();
    requireValue(stat.isDirectory() && stat.uid===process.getuid() && !(stat.mode&0o077),
      'document_join_directory_invalid');
  } finally {await dir.close();}
  const file=await open(join(directory,name),constants.O_RDONLY|constants.O_NOFOLLOW);
  try {
    const stat=await file.stat();
    requireValue(stat.isFile() && stat.uid===process.getuid() && !(stat.mode&0o077)
      && stat.size>0 && stat.size<=max && (prefix===undefined || stat.size>=prefix),
    'document_join_file_invalid');
    const result=Buffer.alloc(prefix??stat.size);let offset=0;
    while(offset<result.length) {
      const {bytesRead}=await file.read(result,offset,result.length-offset,offset);
      requireValue(bytesRead>0,'document_join_file_short');offset+=bytesRead;
    }
    return result;
  } finally {await file.close();}
}

export function journalPrefix(bytes,session) {
  const summary=session?.journal;
  requireValue(summary?.schema==='migration-agent-journal-summary-v1'
    && Number.isSafeInteger(summary.bytes) && summary.bytes>0 && summary.bytes<=64*1024**2
    && Number.isSafeInteger(summary.records) && summary.records>0 && summary.records<=65536
    && checksum(summary.tail_sha256) && bytes.length===summary.bytes && bytes.at(-1)===10,
  'document_join_journal_summary_invalid');
  const lines=bytes.toString('utf8').split('\n');lines.pop();
  requireValue(lines.length===summary.records,'document_join_journal_count');
  let previous=null,last;
  for(const [index,line] of lines.entries()) {
    requireValue(Buffer.byteLength(line)<=4*1024**2+4096,'document_join_record_bound');
    const row=JSON.parse(line),{sha256,...body}=row;
    requireValue(row.schema==='migration-agent-journal-v1' && row.sequence===index
      && row.previous===previous && checksum(sha256) && hash(body)===sha256,
    'document_join_journal_chain');
    previous=sha256;last=row;
  }
  requireValue(previous===summary.tail_sha256 && last.event?.type==='session_stage_result'
    && last.event.result?.stage===session.stage && same(last.event.result.artifact,session.result?.artifact)
    && last.event.result.outcome==='API_JOURNEY_OBSERVED'
    && hash(last.resources)===session.resources_sha256,
  'document_join_session_mismatch');
  return last.resources;
}

function rowFor(snapshot,collection,id) {
  requireValue(identity(id) && Array.isArray(snapshot.collections?.[collection]),'document_join_id_invalid');
  const rows=snapshot.collections[collection];
  const indices=rows.flatMap((row,i)=>row?._id===id?[i]:[]);
  requireValue(indices.length===1,'document_join_row_missing_or_duplicate');
  const index=indices[0],proof=snapshot.document_hashes?.[collection]?.[index];
  // String IDs have identical canonical JSON and BSON Extended JSON encodings.
  requireValue(checksum(proof?.document_sha256) && proof?.id_sha256===hash(id),
    'document_join_original_hash_missing');
  return {row:rows[index],proof};
}

function aggregate(snapshot,collection,id,owner,fields={}) {
  const found=rowFor(snapshot,collection,id),row=found.row;
  requireValue(row.owner===owner && row.record?.id===id && row.record.owner===owner
    && Object.entries(fields).every(([key,value])=>row[key]===value && row.record[key]===value),
  'document_join_aggregate_identity_mismatch');
  return found;
}

export function joinDocuments(snapshot,resources,{stage,selected,owner}) {
  requireValue(STAGES.includes(stage) && identity(owner) && same(artifact(selected),selected)
    && snapshot?.schema==='migration-agent-documents-v1' && snapshot.database==='matrix'
    && Array.isArray(resources) && resources.length===(STAGES.indexOf(stage)<2?1:2),
  'document_join_scope_invalid');
  requireValue(new Set(resources.map(r=>r.agent_id)).size===resources.length,
    'document_join_resource_duplicate');
  const observations=resources.map((resource,index)=>{
    const lineage=resource.lineage;
    requireValue(resource.writer===(index===0?'old-write':'new-write')
      && identity(resource.operation_id) && lineage?.agent_id===resource.agent_id
      && ['sandbox_id','workspace_id','worktree_id'].every(key=>identity(lineage[key])),
    'document_join_resource_invalid');
    const op=rowFor(snapshot,'agent_lifecycle_operations',resource.operation_id);
    requireValue(op.row.owner===owner && op.row.agent_id===resource.agent_id
      && op.row.sandbox_id===lineage.sandbox_id && op.row.status==='succeeded',
    'document_join_lifecycle_mismatch');
    const fields=Object.fromEntries(['sandbox_id','workspace_id','worktree_id'].map(k=>[k,lineage[k]]));
    const agent=aggregate(snapshot,'agents',resource.agent_id,owner,fields);
    const sandbox=aggregate(snapshot,'sandboxes',lineage.sandbox_id,owner);
    const workspace=aggregate(snapshot,'workspaces',lineage.workspace_id,owner,{sandbox_id:lineage.sandbox_id});
    const worktree=aggregate(snapshot,'worktrees',lineage.worktree_id,owner,
      {workspace_id:lineage.workspace_id,sandbox_id:lineage.sandbox_id});
    requireValue(worktree.row.record.path===lineage.worktree_path
      && agent.row.record.created_at===lineage.created_at,'document_join_lineage_mismatch');
    return {agent_sha256:hash(resource.agent_id),writer:resource.writer,lineage_sha256:hash(lineage),
      documents:Object.fromEntries([op,agent,sandbox,workspace,worktree].map((found,i)=>[COLLECTIONS[i],{
        ...found.proof,protected_shape_sha256:hash(found.row),
        schema:found.row.document_schema_version??null}]))};
  });
  return {schema:'migration-agent-document-join-v1',stage,artifact:selected,
    resources:observations,joined_collections:COLLECTIONS,outcome:'JOURNAL_IDENTITIES_JOINED',
    compatibility:'NOT_PROVEN',
    limits:['Identity joins do not prove all nested shapes or all collection decoders.',
      'Response and transcript storage joins remain unproven.',
      'Protected fields retain original hashes, not readable credential bytes.',
      'Majority reads are per collection, not a cross-collection transaction.']};
}

export async function captureJoin(input) {
  const {directory,stage,selected,owner,session,snapshot_sha256}=input;
  requireValue(typeof directory==='string' && resolve(directory)===directory && STAGES.includes(stage)
    && session?.stage===stage && same(session.result?.artifact,selected) && checksum(snapshot_sha256)
    && Number.isSafeInteger(session.journal?.bytes) && session.journal.bytes>0
    && session.journal.bytes<=64*1024**2,'document_join_input_invalid');
  const journal=await privateBytes(directory,'agent-session.private.jsonl',64*1024**2,session.journal.bytes);
  const resources=journalPrefix(journal,session);
  const raw=await privateBytes(directory,'agent-documents-'+stage+'.private.json',32*1024**2);
  requireValue(sha(raw)===snapshot_sha256,'document_join_snapshot_changed');
  return {...joinDocuments(JSON.parse(raw),resources,{stage,selected,owner}),
    snapshot_sha256,session_tail_sha256:session.journal.tail_sha256,resources_sha256:hash(resources)};
}

export async function main(input=process.stdin,output=process.stdout) {
  try {
    const chunks=[];let bytes=0;
    for await(const chunk of input) {
      bytes+=Buffer.byteLength(chunk);requireValue(bytes<=1024**2,'document_join_input_bound');chunks.push(chunk);
    }
    const result=await captureJoin(JSON.parse(Buffer.concat(chunks).toString('utf8')));
    output.write(JSON.stringify(result)+'\n');return 0;
  } catch {
    output.write(JSON.stringify({outcome:'INCOMPLETE',reason:'document_join_failed'})+'\n');return 1;
  }
}

if(process.argv[1] && resolve(process.argv[1])===fileURLToPath(import.meta.url)) process.exitCode=await main();
