import { test } from 'node:test';
import assert from 'node:assert/strict';
import { decodeSnapshot, usedFraction, credits, pace, groups, views, summary, staleLimit, reset } from '../mods/herdr-kit-usage/model.js';
const now = 10_000_000;
const limit = (x={}) => ({id:'a',window:'5h',tier:null,unit:'percent',used_fraction:0.5,used:null,limit:null,remaining:null,unlimited:false,level:'ok',resets_at_ms:now+3600000,...x});
const provider = (name,limits,extra={}) => ({provider:name,state:'available',error:null,fetched_at_ms:now-1000,stale:false,accounts:[{fetched_at_ms:now-1000,limits}],...extra});
test('groups choose worst constrained account and earliest reset', () => {
 const p=provider('codex',[limit({used_fraction:null}),limit({used_fraction:.9,resets_at_ms:now+2000})]);
 p.accounts.push({fetched_at_ms:now-1000,limits:[limit({used_fraction:.9,resets_at_ms:now+1000})]});
 const g=groups(p,now); assert.equal(g.length,1); assert.equal(g[0].used,.9); assert.equal(g[0].limit.resets_at_ms,now+1000);
});
test('credit units retain precision and only compact large values', () => {
 const l=limit({unit:'credits',used_fraction:null,used:null,limit:2000,remaining:1750});
 assert.equal(usedFraction(l),.125); assert.equal(credits(l),'250 / 2,000 AIC'); assert.equal(credits(l,true),'250 of 2,000 AI credits used');
 assert.equal(credits({...l,used:12000,limit:20000}),'12k / 20k AIC'); assert.equal(credits({...l,used:12000,limit:20000},true),'12,000 of 20,000 AI credits used');
});
test('pace requires enough elapsed time and suppresses stale projections', () => {
 const l=limit({used_fraction:.8,resets_at_ms:now+3600000});
 assert.ok(pace(l,now).runsOutMs > 0); assert.equal(pace(l,now,true),null);
 assert.equal(pace({...l,window:'mystery'},now),null); assert.equal(pace({...l,resets_at_ms:now+4.9*3600000},now),null);
 assert.equal(pace({...l,resets_at_ms:now+4.4*3600000},now).runsOutMs,null);
});
test('fresh known values beat stale higher values in summary', () => {
 const snapshot={providers:[provider('codex',[limit({used_fraction:.99})],{stale:true}),provider('claude',[limit({used_fraction:.3})]),provider('copilot',[],{state:'not_signed_in'})]};
 assert.equal(summary(views(snapshot,now)).name,'Claude');
});
test('unknown and unlimited never masquerade as zero; passed reset is stale', () => {
 assert.equal(usedFraction(limit({used_fraction:null})),null); assert.equal(usedFraction(limit({unlimited:true})),null);
 const l=limit({resets_at_ms:now-1}); assert.equal(staleLimit(l,now-1000,now),true); assert.match(reset(l.resets_at_ms,now),/passed/);
});
test('decoder copies only protocol fields and rejects malformed envelope', () => {
 const s={schema_version:1,generated_at_ms:now,collecting:false,providers:[provider('copilot',[limit({id:'copilot:5h:0',unit:'credits'})])]}; s.providers[0].accounts[0].email='secret';
 assert.equal(decodeSnapshot(JSON.stringify(s)).providers[0].accounts[0].email,undefined);
 assert.throws(()=>decodeSnapshot(JSON.stringify({...s,schema_version:2}))); assert.throws(()=>decodeSnapshot(JSON.stringify({...s,providers:[s.providers[0],s.providers[0]]})));
 assert.throws(()=>decodeSnapshot(JSON.stringify({...s,generated_at_ms:-1})));
 assert.throws(()=>decodeSnapshot(JSON.stringify({...s,providers:[{...s.providers[0],accounts:Array(9).fill(s.providers[0].accounts[0])}]})));
 assert.throws(()=>decodeSnapshot(JSON.stringify({...s,providers:[{...s.providers[0],accounts:[{...s.providers[0].accounts[0],limits:Array(25).fill(s.providers[0].accounts[0].limits[0])}]}]})));
});
