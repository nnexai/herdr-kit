import {test} from 'node:test';
import assert from 'node:assert/strict';
import {registerHooks} from 'node:module';

// Run the real store with the native host boundary replaced, not the store logic.
const hostUrl = 'data:text/javascript,' + encodeURIComponent(`
export const exec = (...args) => globalThis.coordHost.exec(...args);
export const shared = key => globalThis.coordHost.shared(key);
export const setShared = () => {};
export const notify = () => {};
export const watchFolder = () => new Promise(() => {});
`);
registerHooks({resolve(specifier, context, next) {
  return specifier === 'herdr' ? {url:hostUrl, shortCircuit:true} : next(specifier, context);
}});
let generation = 0;
const snapshot = (terminal = 'old', state = 'running', pending = false) => ({
  schema_version:1, generated_at_ms:Date.now(), release:'test', state,
  enabled:true, autostart:false, degraded:[],
  watcher:{running:true,terminal:'watcher',heartbeat_ms:Date.now(),release:'test'},
  coordinator:{terminal,status:'working',space_id:'sp',tab_id:'tab',session_started_ms:1,restart_pending:pending},
  counts:{running:0,needs_you:0,retained:0,open_workstreams:0}, journal_dir:'/journal',journal_seq:1,
});
async function setup(initial = snapshot()) {
  const queue = [], calls = [];
  let current = initial;
  globalThis.coordHost = {
    shared: () => current,
    exec: async (_binary, argv) => {calls.push(argv); return {code:0,stdout:JSON.stringify(argv.includes('restart') ? {state:'requested'} : current)};},
  };
  const {store} = await import(`../mods/kit-coord/store.js?test=${++generation}`);
  const cx = {notify(){}, spawn(fn){queue.push(() => fn(cx));}, timer:{after(){throw new Error('unexpected deferred refresh');}}};
  const drain = async () => {while(queue.length) await queue.shift()();};
  store.init();
  return {store,cx,calls,drain,set(value){current=value;}};
}
test('pending request survives store reload and status refresh, allowing confirmed force', async () => {
  const h = await setup(snapshot('old','running',true));
  assert.equal(h.store.restartPending,true);
  h.store.refresh(h.cx,'status',true); await h.drain();
  assert.equal(h.store.restartPending,true);
  assert.equal(h.store.restartMessage,'Will restart when idle.');
  h.store.restart(h.cx,true); await h.drain();
  assert.ok(h.calls.some(args => args.join(' ') === 'restart --force'));
});
test('lost or unhealthy replacement never reports restart success; healthy replacement does', async () => {
  const h = await setup();
  h.store.restart(h.cx); await h.drain();
  for (const [terminal,state,pending] of [[null,'degraded',true],[null,'degraded',false],['new','degraded',false]]) {
    h.set(snapshot(terminal,state,pending)); h.store.refresh(h.cx,'status',true); await h.drain();
    assert.doesNotMatch(h.store.restartMessage,/Coordinator restarted/);
  }
  h.set(snapshot('new')); h.store.refresh(h.cx,'status',true); await h.drain();
  assert.equal(h.store.restartPending,false);
  assert.equal(h.store.restartMessage,'Coordinator restarted.');
});
test('force without pending and clearing context while off have no effect', async () => {
  const h = await setup();
  h.store.restart(h.cx,true); await h.drain();
  h.store.status = snapshot(null,'not_running');
  h.store.restart(h.cx); await h.drain();
  assert.deepEqual(h.calls,[]);
});
