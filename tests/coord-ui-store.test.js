import {test} from 'node:test';
import assert from 'node:assert/strict';
import {registerHooks} from 'node:module';
const hostUrl = 'data:text/javascript,' + encodeURIComponent(`
  export const exec = (...args) => globalThis.railHost(...args);
  export const appHome = () => globalThis.railAppHome || '/tmp/fake-app-home';
  export const checking = () => false;
`);
const fsUrl = 'data:text/javascript,' + encodeURIComponent(`
  export const mkdir = async () => {};
  export const readFile = async p => globalThis.railFsRead ? globalThis.railFsRead(p) : new Uint8Array([137,80,78,71]);
  export const writeFile = async (p, b) => { globalThis.railFsWrites?.push({path:p, len:b.length}); };
  export const readdir = async () => [];
  export const unlink = async p => { globalThis.railFsUnlinks?.push(p); };
`);
registerHooks({resolve(specifier,context,next) {
  if (specifier === 'herdr') return {url:hostUrl,shortCircuit:true};
  if (specifier === 'fs/promises') return {url:fsUrl,shortCircuit:true};
  return next(specifier,context);
}});
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
test('answer options send directly without overwriting an unrelated draft',async () => {
  const h = setup('Unrelated draft');
  h.store.send(h.cx,h.view,{text:'Yes',question:{id:'q2'}}); await h.drain();
  assert.deepEqual(h.calls,[['say','--text','Yes','--question','q2']]);
  assert.equal(h.draft(),'Unrelated draft');
});
test('board includes cleaned workstreams and journal pages use before-seq',async () => {
  const h = setup();
  globalThis.railHost = async (_binary,args) => {
    h.calls.push(args);
    if (args[0] === 'board') return {code:0,stdout:JSON.stringify({schema_version:1,workstreams:[],open_questions:[]})};
    return {code:0,stdout:JSON.stringify({schema_version:1,entries:[{seq:1},{seq:2}],has_more:false})};
  };
  await h.store.read('board');
  h.store.journal = {entries:[{seq:3},{seq:4}],has_more:true};
  h.store.loadOlder(h.cx,h.view); await h.drain();
  assert.deepEqual(h.calls,[['board','--json','--all'],['log','--json','--limit','200','--before-seq','3']]);
  assert.deepEqual(h.store.journal.entries.map(e => e.seq),[1,2,3,4]);
  assert.equal(h.store.journal.has_more,false);
});
test('journal retains at most 500 entries while paging backwards',async () => {
  const h = setup();
  h.store.journal = {entries:Array.from({length:400},(_,i) => ({seq:i+201})),has_more:true};
  globalThis.railHost = async () => ({code:0,stdout:JSON.stringify({schema_version:1,entries:Array.from({length:200},(_,i) => ({seq:i+1})),has_more:false})});
  h.store.loadOlder(h.cx,h.view); await h.drain();
  assert.equal(h.store.journal.entries.length,500);
  assert.equal(h.store.journal.entries[0].seq,1);
  assert.equal(h.store.journal.entries[499].seq,500);
});

test('roles load and mutations go exclusively through kit-coord CLI and surface validation errors',async () => {
  const h = setup();
  let list = [{name:'agy-default',source:'builtin',harness:'agy',model:'gemini-4-neon-high',effort:null,body:'Default'}];
  globalThis.railHost = async (_binary,args) => {
    h.calls.push(args);
    if (args.length === 1 && args[0] === 'roles') return {code:0,stdout:JSON.stringify({roles:list})};
    if (args[0] === 'roles' && args[1] === 'create' && args[2] === 'bad') {
      return {code:3,stderr:JSON.stringify({error:{code:'role_invalid',message:'role start command must reference {brief}'}})};
    }
    if (args[0] === 'roles' && args[1] === 'create') {
      const created = {name:args[2],source:'file',harness:'agy',model:'gemini-4-neon-medium',effort:'low',body:'Fast'};
      list = [...list,created];
      return {code:0,stdout:JSON.stringify({action:'created',role:created})};
    }
    return {code:0,stdout:JSON.stringify({action:'ok'})};
  };
  h.store.loadRoles(h.cx,h.view); await h.drain();
  assert.deepEqual(h.calls,[['roles']]);
  assert.equal(h.store.roles.length,1);

  let createdName = null;
  h.store.mutateRole(h.cx,h.view,['roles','create','agy-fast','--from','agy-default'],{
    notice:'Created role agy-fast.',
    onSuccess:outcome => { createdName = outcome.role.name; },
  });
  await h.drain();
  assert.equal(createdName,'agy-fast');
  assert.equal(h.store.roles.length,2);
  assert.equal(h.store.rolesError,'');
  assert.match(h.store.rolesNotice,/Created role agy-fast/);

  h.store.mutateRole(h.cx,h.view,['roles','create','bad','--harness','agy'],{});
  await h.drain();
  assert.equal(h.store.rolesError,'role start command must reference {brief}');
  assert.equal(h.store.savingRole,false);
});

test('artifacts list, text viewer, inline image preview, open/reveal and cleanup stay lazy and bounded',async () => {
  const h = setup();
  globalThis.railFsWrites = [];
  globalThis.railFsUnlinks = [];
  globalThis.railFsRead = () => new Uint8Array([0x89,0x50,0x4e,0x47,0x0d,0x0a,0x1a,0x0a]);
  const pngAtt = {id:'ws-abc234-t1-a1',ws:'ws-abc234',task:'ws-abc234-t1',label:'shot.png',kind:'image',mode:'staged',bytes:1234,availability:'available',resolved_path:'/data/herdr-kit/coord/artifacts/ws-abc234/ws-abc234-t1/a1-shot.png'};
  const logAtt = {id:'ws-abc234-t1-a2',ws:'ws-abc234',task:'ws-abc234-t1',label:'check.log',kind:'log',mode:'staged',bytes:42,availability:'available',resolved_path:'/data/herdr-kit/coord/artifacts/ws-abc234/ws-abc234-t1/a2-check.log'};
  globalThis.railHost = async (_binary,args) => {
    h.calls.push(args);
    if (args[0] === 'artifacts' && args[1] === 'list') {
      return {code:0,stdout:JSON.stringify({schema_version:1,attachments:[pngAtt,logAtt]})};
    }
    if (args[0] === 'artifact' && args[1] === 'read') {
      return {code:0,stdout:JSON.stringify({schema_version:1,id:logAtt.id,text:'ok line 1\nok line 2',total_bytes:42,shown_bytes:42,cap_bytes:65536,truncated:false,resolved_path:logAtt.resolved_path})};
    }
    if (args[0] === 'artifact' && args[1] === 'preview') {
      return {code:0,stdout:JSON.stringify({schema_version:1,id:pngAtt.id,label:'shot.png',format:'png',ext:'png',width:320,height:200,bytes:1234,cache_key:'ws-abc234-t1-a1-deadbeef',resolved_path:pngAtt.resolved_path})};
    }
    if (args[0] === 'artifact' && args[1] === 'open') {
      return {code:0,stdout:JSON.stringify({action:'opened',id:pngAtt.id})};
    }
    return {code:0,stdout:JSON.stringify({action:'ok'})};
  };

  // Lazy list
  h.store.loadWorkstreamArtifacts(h.cx,h.view,'ws-abc234');
  await h.drain();
  assert.deepEqual(h.calls[0],['artifacts','list','--ws','ws-abc234','--json']);
  assert.equal(h.store.wsArtifactMap('ws-abc234').get(pngAtt.id).label,'shot.png');

  // Inline inert plain-text preview (no nested page switch)
  h.view.page = 'stream';
  h.store.loadTextPreview(h.cx,h.view,logAtt);
  await h.drain();
  assert.equal(h.view.page,'stream');
  assert.equal(h.store.textPreviews.get(logAtt.id).status,'ready');
  assert.equal(h.store.textPreviews.get(logAtt.id).text,'ok line 1\nok line 2');

  // Inline image preview stages ephemeral mod-data file and cleans up on clear/dispose
  h.store.loadImagePreview(h.cx,h.view,pngAtt);
  await h.drain();
  const prev = h.store.imagePreviews.get(pngAtt.id);
  assert.equal(prev.status,'ready');
  assert.match(prev.relPath,/^data\/kit-coord-ui\/preview\/p-\d+-ws-abc234-t1-a1-deadbeef\.png$/);
  assert.equal(globalThis.railFsWrites.length,1);

  h.store.clearImagePreview(h.cx,pngAtt.id);
  await h.drain();
  assert.equal(h.store.imagePreviews.size,0);
  assert.equal(globalThis.railFsUnlinks.length,1);

  await h.store.disposePreviews();
  assert.equal(h.store.textPreviews.size,0);
});
