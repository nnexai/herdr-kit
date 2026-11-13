import {test} from 'node:test';
import assert from 'node:assert/strict';
import {registerHooks} from 'node:module';

// Record the rendered native tree; actual view, selection and action logic run below.
const host = 'data:text/javascript,' + encodeURIComponent(`
export class View {}
export const element = (kind, id) => {
  const node = {kind, attrs: id ? {id:[id]} : {}, children:[], events:{}};
  const proxy = new Proxy(node, {get(target, key) {
    if (key in target) return target[key];
    if (key === 'child') return child => {target.children.push(child); return proxy;};
    return (...args) => { (key.startsWith('on_') ? target.events : target.attrs)[key] = args; return proxy; };
  }});
  return proxy;
};
export const div = () => element('div');
export const svg = () => element('svg');
export const h_flex = () => element('h_flex');
export const v_flex = () => element('v_flex');
export const Button = {new:id => element('button',id)};
export const Input = {new:() => element('input')};
export const ScrollHandle = {new:() => ({scroll_to_item(){}})};
export const InputState = {new:() => ({})};
export const theme = () => new Proxy({}, {get:(_, key) => key});
export const props = () => ({overlay:'popup'});
export const agents = () => [];
export const openOverlay = () => 'popup';
export const closeOverlay = id => globalThis.popupCalls.push(['close',id]);
export const run = (name,args) => globalThis.popupCalls.push([name,args]);
export const shared = () => null;
export const setShared = () => {};
export const notify = () => {};
export const exec = () => {throw new Error('unexpected process invocation');};
export const nextCommand = () => new Promise(() => {});
export const watchFolder = () => new Promise(() => {});
`);
registerHooks({resolve(specifier, context, next) {
  return ['gpui-kit','gpui-base','herdr'].includes(specifier) ? {url:host,shortCircuit:true} : next(specifier,context);
}});
const {default:Coordinator} = await import('../mods/herdr-kit-coord/main.js');
const {store} = await import('../mods/herdr-kit-coord/store.js');
const now = 1763042861000;
const status = {enabled:true,state:'running',degraded:[],watcher:{running:true},coordinator:{terminal:'coord'},generated_at_ms:now};
const task = {id:'w-t1',title:'Task with a long title '.repeat(8),state:'running',terminal:'worker',space_id:'space',tab_id:'tab',rejected_rungs:[],rung:2,number:1,last_event_ms:now,state_since_ms:now};
const workstream = {id:'w',title:'Workstream with a long title '.repeat(8),state:'open',repo:{name:'repo'},space:{id:'space'},tasks:[task],last_event_ms:now};
const board = {workstreams:[workstream],attention:[],generated_at_ms:now};
const log = {entries:[{seq:1,kind:'decision',actor:'person',ts_ms:now,ws:'w',task:'w-t1',data:{what:'A decision'}}],generated_at_ms:now};
const cx = {notify(){},stop_propagation(){},write_to_clipboard(text){globalThis.popupCalls.push(['copy',text]);}};
function setup(width = 760, selectedKey = 'w') {
  globalThis.popupCalls = [];
  Object.assign(store,{status,board,log,failed:false,boardFailed:false,logFailed:false,starting:false,restartPending:false,restarting:false,restartMessage:'',refresh(){},setScope(scope){this.scope=scope;}});
  return Object.assign(new Coordinator(),{details:true,width,store,overlayId:'popup',focus:{focus(){}},scroll:{scroll_to_item(){}},ui:{view:'board',selectedKey,expanded:new Set(),collapsed:new Set(),showClosed:new Set(),scope:{},text:'',detail:'key',now,liveAgents:new Map()}});
}
const nodes = tree => typeof tree === 'object' && tree ? [tree,...tree.children.flatMap(nodes)] : [];
const find = (tree,id) => nodes(tree).find(node => node.attrs.id?.[0] === id);
const click = (tree,id) => {const button=find(tree,id); assert.ok(button,`reachable action ${id}`); assert.ok(button.attrs.tab_index[0]>0); button.events.on_click[0]({},cx);};
const popup = view => find(view.render(),'coord-dialog');
const chrome = tree => {
  const simplify = node => typeof node !== 'object' ? node : {
    kind:node.kind,attrs:Object.fromEntries(Object.entries(node.attrs).filter(([key]) => !['hover','selected','bg'].includes(key))),children:node.children.map(simplify),
  };
  return {frame:tree.attrs,header:simplify(tree.children[0]),footer:Object.fromEntries(Object.entries(tree.children.at(-1).attrs).filter(([key])=>key!=='tooltip'))};
};

test('tab switches preserve shared chrome and coordinator controls at normal and narrow widths',()=>{
  for (const width of [760,400]) for (const state of ['running','not_running','degraded']) {
    const view=setup(width);
    store.status={...status,state,enabled:state!=='not_running',degraded:state==='degraded'?[{code:'watcher_missing'}]:[]};
    const before=popup(view);
    click(before,'coord-tab-timeline');
    const after=popup(view);
    assert.deepEqual(chrome(after),chrome(before));
    if (state!=='running') assert.equal(nodes(before).filter(n=>n.attrs.id?.[0]==='coord-start').length,1);
  }
});

test('actions stay on their selected Board row and execute against that row',()=>{
  const view=setup();
  let tree=popup(view), row=find(tree,'coord-row-w');
  for (const id of ['selected-toggle','selected-show-space','selected-log','selected-details']) assert.ok(find(row,id));
  assert.equal(nodes(tree).filter(n=>n.attrs.role?.[0]==='toolbar').length,1);
  click(tree,'selected-toggle'); assert.ok(view.ui.collapsed.has('w'));
  click(popup(view),'selected-toggle'); assert.ok(!view.ui.collapsed.has('w'));
  click(popup(view),'selected-show-space'); assert.deepEqual(globalThis.popupCalls.at(-1),['space.select',{id:'space'}]);
  view.ui.selectedKey='w-t1'; tree=popup(view); row=find(tree,'coord-row-w-t1');
  for (const id of ['selected-show','selected-peek','selected-log','selected-details']) assert.ok(find(row,id));
  assert.equal(find(find(tree,'coord-row-w'),'selected-log'),undefined);
  click(tree,'selected-details'); assert.ok(view.ui.expanded.has('w-t1'));
  click(popup(view),'selected-peek'); assert.deepEqual(globalThis.popupCalls.at(-1),['terminal.peek',{terminal:'worker'}]);
  click(popup(view),'selected-show'); assert.deepEqual(globalThis.popupCalls.at(-1),['agent.focus',{terminal:'worker'}]);
  click(popup(view),'selected-log'); assert.equal(view.ui.view,'timeline'); assert.deepEqual(view.ui.scope,{task:'w-t1'});
});

test('selected Timeline actions retain details, copy and scope behavior',()=>{
  const view=setup(); view.ui.view='timeline';
  let tree=popup(view);
  const row=nodes(tree).find(n=>n.attrs.id?.[0]?.startsWith('timeline-row-'));
  view.ui.selectedKey=row.attrs.id[0].slice('timeline-row-'.length); tree=popup(view);
  click(tree,'selected-details'); assert.ok(view.ui.expanded.has(view.ui.selectedKey));
  click(popup(view),'selected-copy'); assert.match(globalThis.popupCalls.at(-1)[1],/A decision/);
  click(popup(view),'selected-scope'); assert.deepEqual(view.ui.scope,{task:'w-t1'});
});
