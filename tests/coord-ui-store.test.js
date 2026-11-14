import {test} from 'node:test';
import assert from 'node:assert/strict';
import {registerHooks} from 'node:module';
const hostUrl = 'data:text/javascript,' + encodeURIComponent('export const exec = (...args) => globalThis.railHost(...args);');
registerHooks({resolve(specifier,context,next) { return specifier === 'herdr' ? {url:hostUrl,shortCircuit:true} : next(specifier,context); }});
const {RailStore} = await import('../mods/kit-coord-ui/store.js');
function setup(value = '  exact\n\tmessage  ') {
  const store = new RailStore(), queue = [], calls = [];
  let draft = value;
  const cx = {notify(){},spawn(fn){queue.push(() => fn(cx));}};
  const view = {closed:false,draft,question:{id:'q1'},input:{value:() => draft,set_value(value){draft=value;}},
    model:() => ({available:true,canSend:true,working:false}),didRefresh(){}};
  store.refresh = () => {};
  globalThis.railHost = async (binary,args) => {calls.push(args); return {code:0,stdout:JSON.stringify({state:'sent'})};};
  return {store,view,cx,calls,drain:async () => {while(queue.length) await queue.shift()();},draft:() => draft,setDraft(value){draft=value;view.draft=value;}};
}
test('confirmed send targets only coordinator say and clears the delivered draft',async () => {
  const h = setup(); h.store.send(h.cx,h.view); await h.drain();
  assert.deepEqual(h.calls,[['say','--text','  exact\n\tmessage  ','--question','q1']]);
  assert.equal(h.draft(),''); assert.equal(h.view.question,null);
});
test('failed delivery retains the exact draft and does not retry automatically',async () => {
  const h = setup('Keep this');
  globalThis.railHost = async (_binary,args) => {h.calls.push(args);return {code:4,stderr:'coordinator_changed: coordinator session changed'};};
  h.store.send(h.cx,h.view); await h.drain();
  assert.equal(h.draft(),'Keep this'); assert.equal(h.calls.length,1);
  assert.match(h.store.notice,/coordinator_changed/); assert.equal(h.store.sending,false);
});
test('double submit and busy state do not deliver duplicates',async () => {
  const h = setup(); h.store.send(h.cx,h.view); h.store.send(h.cx,h.view); await h.drain();
  assert.equal(h.calls.length,1);
  h.setDraft('Wait'); h.view.model = () => ({available:true,canSend:false,working:true});
  h.store.send(h.cx,h.view); await h.drain(); assert.equal(h.calls.length,1);
});
test('editing during delivery keeps the new draft; confirmed old-session delivery never suggests retry',async () => {
  const h = setup('Old draft');
  globalThis.railHost = async () => {h.setDraft('New draft');return {code:0,stdout:JSON.stringify({state:'sent',coordinator_changed:true,answered:false})};};
  h.store.send(h.cx,h.view); await h.drain();
  assert.equal(h.draft(),'New draft'); assert.match(h.store.notice,/previous session/);
});
