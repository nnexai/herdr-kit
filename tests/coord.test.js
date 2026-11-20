import {test} from 'node:test';
import assert from 'node:assert/strict';
import * as m from '../mods/kit-coord/model.js';
const now=1763042861000;
const status=(x={})=>({schema_version:1,generated_at_ms:now,release:'a1b2c3d4',state:'running',enabled:true,autostart:true,degraded:[],watcher:{running:true,terminal:'t5',heartbeat_ms:now-4000,release:'a1b2c3d4'},coordinator:{terminal:'t4',status:'working',space_id:'sp',tab_id:'tb',session_started_ms:now-1000},counts:{running:3,needs_you:1,retained:2,open_workstreams:3},journal_dir:'/journal',journal_seq:42,...x});
const json=JSON.stringify;
test('status decoder preserves protocol and enforces caps',()=>{ const value=status(); assert.deepEqual(m.decodeStatus(json(value)),value); assert.throws(()=>m.decodeStatus(json({...value,schema_version:2})),/malformed/); assert.throws(()=>m.decodeStatus(json({...value,generated_at_ms:-1})),/malformed/); assert.throws(()=>m.decodeStatus('x'),/malformed/); const s=status({email:'private',state:'future'}); assert.equal(m.decodeStatus(json(s)).email,undefined); assert.equal(m.decodeStatus(json(s)).state,'future'); assert.throws(()=>m.decodeStatus(json(status({degraded:Array(201).fill({code:'x',since_ms:null,detail:null})})))); });
test('status bar states, widths, stale and actions',()=>{ const vm=(s,c={})=>m.statusbarModel(s,{now,width:280,...c}); assert.equal(vm(status()).text,'3 running · 1 needs you · 2 retained'); assert.equal(vm(status()).tier,'full'); assert.equal(vm(status(),{width:150}).tier,'dots'); assert.equal(vm(status(),{width:40}).tier,'icon'); assert.equal(vm(status({counts:{running:999,needs_you:999,retained:999,open_workstreams:1}})).tier,'dots'); assert.equal(vm(status({counts:{running:0,needs_you:0,retained:0,open_workstreams:0}})).text,'No tasks'); assert.equal(vm(status({state:'not_running'})).click,'up'); assert.match(vm(status({state:'degraded',degraded:[{code:'watcher_missing',since_ms:null,detail:null}]})).text,/1 needs you/); assert.equal(vm(status(),{starting:true}).click,'none'); assert.equal(vm(null).click,'reread'); assert.equal(vm(status()).click,'focus'); assert.match(vm(status(),{failed:true,lastOkMs:now-100000}).text,/ ~$/); assert.match(vm(status()).tooltip,/Click to focus the coordinator/); });
test('3 autostart predicate',()=>{ assert.equal(m.shouldAutostart(status({state:'degraded'}),false),true); for(const s of [status({enabled:false,state:'degraded'}),status({autostart:false,state:'degraded'}),status()]) assert.equal(m.shouldAutostart(s,false),false); assert.equal(m.shouldAutostart(status({state:'degraded'}),true),false); });

test('status strip exposes every state and its accessible text',()=>{
 const vm=(s,c={})=>m.statusbarModel(s,{now,width:500,...c});
 const zero={running:0,needs_you:0,retained:0,open_workstreams:0};
 assert.equal(vm(status({counts:zero})).text,'No tasks');
 assert.equal(vm(status()).text,'3 running · 1 needs you · 2 retained');
 assert.equal(vm(status({state:'not_running'})).text,'Coordinator off');
 assert.match(vm(status({enabled:false})).tooltip,/creates a "coordinator" space/);
 const degraded=status({state:'degraded',degraded:[{code:'watcher_heartbeat_stale',detail:'watcher heartbeat stale for 3m 20s',since_ms:null},{code:'hc_unreachable',detail:null,since_ms:null}]});
 assert.equal(vm(degraded).text,'Degraded · watcher stalled +1 · 1 needs you');
 assert.match(vm(degraded).tooltip,/watcher heartbeat stale for 3m 20s/);
 assert.equal(vm(status(),{starting:true}).text,'Starting…');
 assert.equal(vm(null).text,'Coordinator …');
 assert.equal(vm(null,{failed:true,error:'read failed\nmore'}).text,'Coordinator unavailable');
 assert.equal(vm(null,{failed:true,error:'read failed\nmore'}).tooltip,'Coordinator status unavailable: read failed. Click to retry.');
 const stale=vm(status({generated_at_ms:now-100000}),{failed:true});
 assert.match(stale.text,/ ~$/);
 assert.match(stale.compactText,/ ~$/);
 assert.match(stale.tooltip,/ Last updated \d\d:\d\d\.$/);
 assert.equal(vm(status({generated_at_ms:now-121000}),{failed:true}).state,'unavailable');
});

test('attention remains visible on the compact bar',()=>{
 for (const state of ['running','degraded','not_running']) {
  const small=m.statusbarModel(status({state}),{now,width:80});
  assert.equal(small.iconRole,'blocked');
  if (state === 'running') { assert.equal(small.tier,'dots'); assert.deepEqual(small.segments.map(s=>s.key),['needs_you']); assert.equal(small.compactText,'● 1'); }
  assert.notEqual(m.statusbarModel(status({state,counts:{running:0,needs_you:0,retained:0}}),{now,width:80}).iconRole,'blocked');
 }
});

