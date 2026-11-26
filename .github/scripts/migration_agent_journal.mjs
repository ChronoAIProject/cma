import {constants} from 'node:fs';
import {open,realpath} from 'node:fs/promises';
import {join,resolve} from 'node:path';
import {hash,requireValue} from './migration_agent_observations.mjs';

export class PrivateAgentJournal {
  #file; #secrets; #sequence=0; #previous=null; #bytes=0; #active=false; #closed=false;
  #poisoned=false;
  constructor(file,secrets) {this.#file=file;this.#secrets=secrets;}
  static async create(directory,secrets) {
    requireValue(Array.isArray(secrets) && secrets.every(s=>typeof s==='string' && s.length>=16),
      'journal_redaction_contract');
    const path=resolve(directory);
    requireValue(await realpath(path)===path,'private_campaign_directory_required');
    const dir=await open(path,constants.O_RDONLY|constants.O_NOFOLLOW);
    try {
      const stat=await dir.stat();
      requireValue(stat.isDirectory() && stat.uid===process.getuid() && !(stat.mode&0o077),
        'private_campaign_directory_required');
    } finally {await dir.close();}
    const file=await open(join(path,'agent-session.private.jsonl'),
      constants.O_WRONLY|constants.O_CREAT|constants.O_EXCL|constants.O_NOFOLLOW,0o600);
    return new PrivateAgentJournal(file,[...secrets].sort((a,b)=>b.length-a.length));
  }
  async append(event,resources=[]) {
    requireValue(!this.#closed && !this.#active && !this.#poisoned,'journal_not_writable');
    this.#active=true;
    try {
      let raw=JSON.stringify({event,resources});
      for(const secret of this.#secrets) raw=raw.split(JSON.stringify(secret).slice(1,-1)).join('[REDACTED]');
      requireValue(Buffer.byteLength(raw)<=4*1024*1024,'journal_event_bound');
      const record={schema:'migration-agent-journal-v1',sequence:this.#sequence,previous:this.#previous,
        ...JSON.parse(raw)};
      const checksum=hash(record),line=JSON.stringify({...record,sha256:checksum})+'\n';
      requireValue(this.#bytes+Buffer.byteLength(line)<=64*1024*1024,'journal_total_bound');
      try {await this.#file.writeFile(line);await this.#file.sync();}
      catch(error) {this.#poisoned=true;throw error;}
      this.#bytes+=Buffer.byteLength(line);this.#previous=checksum;this.#sequence++;
      return this.summary();
    } finally {this.#active=false;}
  }
  summary() {return {schema:'migration-agent-journal-summary-v1',records:this.#sequence,
    tail_sha256:this.#previous,bytes:this.#bytes};}
  async close() {
    requireValue(!this.#active,'journal_write_in_progress');
    if(!this.#closed) {this.#closed=true;await this.#file.close();}
    return this.summary();
  }
}
