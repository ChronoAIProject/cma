// Majority reads from the exclusive synthetic database. No mutation authority.
'use strict';
const {createHash}=require('node:crypto');

const COLLECTIONS = Object.freeze([
  'agent_command_orders','agent_commands','agent_creation_provenance','agent_lifecycle_operations',
  'agent_log','agent_log_admissions','agent_log_audits','agent_log_evidence','agent_log_gates',
  'agent_log_heads','agent_log_projections','agent_log_publications','agent_log_segments','agents',
  'cma_agent_manager_members','cma_agent_manager_owners',
  'sandbox_admissions','sandbox_controls','sandboxes','workspaces','worktrees',
]);
const MAX_ROWS=2048,MAX_TOTAL_ROWS=8192,MAX_BYTES=32*1024*1024;
const SENSITIVE=/(?:token|secret|password|bearer|credential|authorization|private_key|keyfile|encryption_key)/i;
const checksum=value=>createHash('sha256').update(value).digest('hex');

function protect(value,serialize,redactions,path=[],sensitive=false) {
  if(value===null || typeof value==='number' || typeof value==='boolean')return value;
  if(sensitive && typeof value==='string') {
    redactions.push({path,canonical_sha256:checksum(serialize(value)),type:'string'});
    return '[REDACTED]';
  }
  if(Array.isArray(value))return value.map((item,i)=>protect(item,serialize,redactions,[...path,i],sensitive));
  if(value && typeof value==='object' && (value._bsontype || value instanceof Date || Buffer.isBuffer(value))) {
    if(!sensitive)return value;
    redactions.push({path,canonical_sha256:checksum(serialize(value)),type:value._bsontype??'binary_or_date'});
    return {redacted_bson_type:value._bsontype??'binary_or_date'};
  }
  if(value && typeof value==='object')return Object.fromEntries(Object.entries(value).map(([key,item])=>
    [key,protect(item,serialize,redactions,[...path,key],sensitive || SENSITIVE.test(key))]));
  return value;
}

function collect(database,serialize) {
  if(typeof serialize!=='function')throw Error('canonical_serializer_required');
  const collections={},document_hashes={},redactions=[];let totalRows=0,totalBytes=0;
  const command=request=>{
    const result=database.runCommand(request);
    if(!result || Number(result.ok)!==1)throw Error('document_read_failed');
    return result;
  };
  for(const name of COLLECTIONS) {
    let cursor=null,liveId=null,rows=[],pages=0,field='firstBatch';const ids=new Set(),hashes=[];
    try {
      cursor=command({find:name,filter:{},sort:{_id:1},limit:MAX_ROWS+1,batchSize:32,
        maxTimeMS:2000,readConcern:{level:'majority'}}).cursor;
      liveId=cursor?.id??null;
      while(true) {
        if(!cursor || cursor.id==null || cursor.ns!=='matrix.'+name
          || !Array.isArray(cursor[field]) || ++pages>MAX_ROWS+1)throw Error('document_cursor_invalid');
        liveId=cursor.id;
        for(const row of cursor[field]) {
          if(!row || typeof row!=='object' || Array.isArray(row) || !Object.hasOwn(row,'_id'))
            throw Error('document_identity_missing');
          const id=serialize(row._id),raw=serialize(row),bytes=Buffer.byteLength(raw);
          if(ids.has(id))throw Error('document_identity_duplicate');
          ids.add(id);totalRows++;totalBytes+=bytes;
          hashes.push({id_sha256:checksum(id),document_sha256:checksum(raw)});
          rows.push(protect(row,serialize,redactions,[name,rows.length]));
          if(rows.length>MAX_ROWS || totalRows>MAX_TOTAL_ROWS || totalBytes>MAX_BYTES)
            throw Error('document_snapshot_bound');
        }
        if(cursor.id.toString()==='0')break;
        cursor=command({getMore:cursor.id,collection:name,batchSize:32,maxTimeMS:2000}).cursor;
        field='nextBatch';
      }
    } finally {
      if(liveId!=null && liveId.toString()!=='0') {
        // The requested collection is fixed; a malformed response cannot make
        // cleanup target a foreign cursor namespace.
        command({killCursors:name,cursors:[liveId]});
      }
    }
    collections[name]=rows;document_hashes[name]=hashes;
  }
  const result={schema:'migration-agent-documents-v1',database:'matrix',collections,document_hashes,redactions,
    consistency:'majority-per-collection; not a cross-collection transaction'};
  if(Buffer.byteLength(serialize(result))>MAX_BYTES)throw Error('document_snapshot_bound');
  return result;
}

if(typeof module!=='undefined')module.exports={collect,protect,COLLECTIONS,MAX_ROWS,MAX_TOTAL_ROWS,MAX_BYTES};
if(typeof process!=='undefined' && process.env.MATRIX_AGENT_READ_URI) {
  let connection;
  try {
    connection=new Mongo(process.env.MATRIX_AGENT_READ_URI);
    const serialize=value=>EJSON.stringify(value,{relaxed:false});
    print(serialize(collect(connection.getDB('matrix'),serialize)));
  } catch {
    print('migration Agent document collection failed');quit(1);
  } finally {if(connection)connection.close();}
}
