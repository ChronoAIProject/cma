"""BSON and authority join checks, including mutations that HTTP alone misses."""
import base64
from copy import deepcopy
import secrets
import unittest

from migration_api_custody_provider import issue, metadata
from migration_api_custody_validation import COLLECTION, SCHEMA, binary, compare, validate
from migration_browser_fixture import OWNER
from test_migration_sandbox_documents import snapshot as commands_snapshot, bson, digest


def encoded(raw):
    return {'$binary': {'base64': base64.b64encode(raw).decode(), 'subType': '00'}}


def snapshot(writers=1):
    controls, records = commands_snapshot(writers)
    rows = []
    for w, record in enumerate(records):
        for i, command in enumerate(record['commands']):
            token = issue(OWNER, 2000000000)
            meta = metadata(token, OWNER, 2000000000)
            command['custody'] = {'slot': f"{record['writer']}/{i}", **meta}
            row = controls['collections']['sandbox_controls'][2*w+i]
            envelope = row['record']['envelope']
            authority = f"api:{meta['custody_id']}:{meta['expires_at']}"
            envelope['authorization']['authority_id'] = authority
            row['scope'] = digest(['sandbox-control-scope-v1', authority, envelope['actor'], row['sandbox_id'], 'pause'])
            rows.append({'_id': encoded(bytes.fromhex(meta['custody_id'])), 'document_schema_version': bson(1),
                **{k:meta[k] for k in ('issuer','subject','credential_id')},
                'expires_at': {'$date': {'$numberLong': str(meta['expires_at']*1000)}},
                'access_token': {'version': bson(1), 'nonce': encoded(secrets.token_bytes(12)),
                                 'ciphertext': encoded(secrets.token_bytes(meta['token_bytes']+16))}})
    return {'schema': SCHEMA, 'database': 'matrix', 'collections': {COLLECTION: rows}}, records, controls


class ValidationTests(unittest.TestCase):
    def test_valid_exact_bson_in_both_writer_directions(self):
        for count in (1,2):
            data, records, controls = snapshot(count)
            self.assertEqual(len(validate(data,records,controls)), count*2)

    def test_empty_foreign_duplicate_and_extra_scope_fail(self):
        for fault in ('empty','duplicate','foreign','extra','database','schema'):
            data,records,controls=snapshot(); rows=data['collections'][COLLECTION]
            if fault=='empty': rows.clear()
            if fault=='duplicate': rows[1]=deepcopy(rows[0])
            if fault=='foreign': rows[0]['_id']=encoded(secrets.token_bytes(32))
            if fault=='extra': data['collections']['auth_subjects']=[]
            if fault in ('database','schema'): data[fault]='wrong'
            with self.subTest(fault=fault),self.assertRaises(RuntimeError): validate(data,records,controls)

    def test_identity_expiry_cipher_and_unknown_field_mutations_fail(self):
        faults=[lambda r:r.update(issuer='http://foreign'),lambda r:r.update(subject='other'),
            lambda r:r.update(credential_id='b'*64),lambda r:r.update(extra=1),
            lambda r:r.update(expires_at=None),lambda r:r.update(document_schema_version=bson(2)),
            lambda r:r.update(expires_at={'$date':{'$numberLong':'2000000000001'}}),
            lambda r:r['access_token'].update(version=bson(2)),
            lambda r:r['access_token'].update(nonce=encoded(b'a'*11)),
            lambda r:r['access_token'].update(ciphertext=encoded(b'plaintext')),
            lambda r:r['access_token'].update(extra='field')]
        for i,mutate in enumerate(faults):
            data,records,controls=snapshot(); mutate(data['collections'][COLLECTION][0])
            with self.subTest(i=i),self.assertRaises(RuntimeError):validate(data,records,controls)

    def test_command_authority_and_saved_metadata_cannot_be_forged(self):
        for field,value in [('slot','new-write/0'),('subject','other'),('credential_id','a'*64),
                            ('custody_id','a'*64),('expires_at',True),('token_bytes',0)]:
            data,records,controls=snapshot(); records[0]['commands'][0]['custody'][field]=value
            with self.subTest(field=field),self.assertRaises(RuntimeError):validate(data,records,controls)
        for suffix in (':none',':2000000001'):
            data,records,controls=snapshot()
            row=controls['collections']['sandbox_controls'][0]; e=row['record']['envelope']
            e['authorization']['authority_id']='api:'+records[0]['commands'][0]['custody']['custody_id']+suffix
            row['scope']=digest(['sandbox-control-scope-v1',e['authorization']['authority_id'],e['actor'],row['sandbox_id'],'pause'])
            with self.assertRaisesRegex(RuntimeError,'authority not bound'):validate(data,records,controls)

    def test_binary_requires_exact_canonical_bson(self):
        for value in (None,{}, {'$binary':{}}, {'$binary':{'base64':'***','subType':'00'}},
                      {'$binary':{'base64':'YQ==','subType':'04'}},
                      {'$binary':{'base64':'YR==','subType':'00'}}, encoded(b'a')):
            with self.subTest(value=value),self.assertRaises(RuntimeError):binary(value,32)
        self.assertEqual(binary(encoded(b'abc')),b'abc')

    def test_opposite_reader_preserves_ciphertext_and_writer_only_adds_two(self):
        data,_,_=snapshot(); old=data['collections'][COLLECTION]
        extra,_,_=snapshot(); new=deepcopy(old)+extra['collections'][COLLECTION]
        compare(old,deepcopy(old),False);compare(old,new,True);compare(new,deepcopy(new),False)
        changed=deepcopy(old);changed[0]['access_token']['nonce']=encoded(b'x'*12)
        for after,writer in ((changed,False),(changed+new[2:],True),([],False),(new,False),(old,True),(old+[old[0]],True)):
            with self.assertRaises(RuntimeError):compare(old,after,writer)


if __name__=='__main__':unittest.main()
