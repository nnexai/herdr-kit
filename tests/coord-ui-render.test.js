import {test} from 'node:test';
import assert from 'node:assert/strict';
import {registerHooks} from 'node:module';

// GPUI descriptions are single-use values: parenting consumes a builder. Model
// this boundary so the rare off-screen-question branch cannot break the view.
function element(kind) {
  const value = {kind,nodes:[],attributes:{},consumed:false};
  let proxy;
  const consume = child => {
    if (!child || typeof child !== 'object' || !child.nodes) return;
    assert.equal(child.consumed,false,'native element already has a parent'); child.consumed=true;
  };
  proxy = new Proxy(value,{get(target,key) {
    if (key in target) return target[key];
    return (...args) => {
      assert.equal(target.consumed,false,`cannot modify parented ${kind}`);
      if (key === 'child' || key === 'trigger' || key === 'content') {consume(args[0]);target.nodes.push(args[0]);}
      else target.attributes[key]=args.length ? args[0] : true;
      return proxy;
    };
  }});
  return proxy;
}
globalThis.railElement = element;
const nativeUrl = 'data:text/javascript,'+encodeURIComponent(`
export const div=()=>globalThis.railElement('div'),svg=()=>globalThis.railElement('svg');
export class View {}
export const ScrollHandle={new:()=>({})},TextareaState={new:()=>({})},InputState={new:()=>({})};
export const h_flex=()=>globalThis.railElement('h_flex'),v_flex=()=>globalThis.railElement('v_flex');
export const Button={new:id=>{const e=globalThis.railElement('button');e.id(id);return e;}},Checkbox={new:id=>{const e=globalThis.railElement('checkbox');e.id(id);return e;}},Textarea={new:()=>globalThis.railElement('textarea')},Input={new:()=>globalThis.railElement('input')},
TextView={markdown:()=>globalThis.railElement('markdown')},Popover={new:()=>globalThis.railElement('popover')};
`);
const hostUrl = 'data:text/javascript,'+encodeURIComponent(`
export const settings=()=>({dark:true});
export const props=()=>({}),shared=()=>null,setShared=()=>{},run=()=>{},openOverlay=()=>'',closeOverlay=()=>{};
export const nextCommand=()=>new Promise(()=>{}),exec=()=>{throw new Error('unexpected native exec in renderer test');};
export const theme=()=>({foreground:'#e6e7e9',muted:'#9a9ea5',accent:'#8fa9dc',accent_foreground:'#14181f',
popover:'#24272b',canvas:'#101214',raised:'#2d3035',hover:'#34373c',selected:'#3b3e44',border:'#44474d',input:'#2a2d31',done:'#6fcf97',working:'#e2b94e',blocked:'#ef6a67'});
`);
registerHooks({resolve(specifier,context,next) {
  const url = ['gpui-kit','gpui-base'].includes(specifier) ? nativeUrl : specifier === 'herdr' ? hostUrl : null;
  return url ? {url,shortCircuit:true} : next(specifier,context);
}});
const {renderRail} = await import('../mods/kit-coord-ui/render.js');
function all(node) {return typeof node === 'object' && node ? [node,...node.nodes.flatMap(all)] : [];}
function fixture(overrides = {}) {
  const vm = {rows:[],questions:[],needsYou:[],available:true,canSend:true,working:false,status:'running',elapsed:'0:00',...overrides};
  const view = {width:680,reducedMotion:true,store:{status:{coordinator:{}},notice:''},rowHeights:new Map(),windowStart:0,
    expandedDays:new Set(),expandedReceipts:new Set(),expandedQuestions:new Set(),clampedQuestions:new Set(),expandedJournal:new Set(),hoveredReceipt:null,
    rootFocus:{},logFocus:{},menuFocus:{},scroll:{},input:{},draft:'',question:null,follow:true,model:()=>vm,key(){},
    toggle(set,key,cx){if (set.has(key)) set.delete(key); else set.add(key); cx.notify();},
    toggleReceipt(key,cx){this.toggle(this.expandedReceipts,key,cx);}};
  return {vm,view};
}
const byId = (nodes,id) => nodes.find(node => node.attributes?.id === id);
test('needs-you is pinned and limited to two, with overflow reachable',() => {
  const question = n => ({kind:'question',questionId:`q${n}`,text:`Question ${n}`,title:'Work',ts_ms:1000,options:['Yes']});
  const {view} = fixture({needsYou:[question(1),question(2),question(3)]});
  const nodes = all(renderRail(view));
  assert.ok(byId(nodes,'needs-0')); assert.ok(byId(nodes,'needs-1'));
  assert.equal(byId(nodes,'needs-2'),undefined); assert.ok(byId(nodes,'needs-all'));
  assert.ok(byId(nodes,'needs-option-0-0')); assert.ok(byId(nodes,'needs-reply-0'));
});
test('receipt actions replace metadata on hover or selection; trace stays scoped',() => {
  const row = {key:'receipt:w',kind:'receipt',title:'Build',text:'Working',state:'running',ts_ms:1000,space:'Repo',duration:'2m',
    trace:[{ts_ms:1000,text:'Placed here'}],links:[{kind:'terminal',target:'t'},{kind:'space',target:'s'}]};
  const {view} = fixture({rows:[row]}), cx = {notify(){},stop_propagation(){}};
  let nodes = all(renderRail(view));
  assert.ok(nodes.some(n => n.nodes?.includes('Repo  2m')));
  assert.ok(nodes.some(n => n.nodes?.includes('Working') && n.attributes?.truncate),'receipt detail stays on one line');
  assert.equal(byId(nodes,'trace-receipt:w'),undefined); assert.equal(byId(nodes,'terminal-receipt:w'),undefined);
  byId(nodes,'receipt:w').attributes.on_hover(true,cx);
  nodes = all(renderRail(view));
  assert.ok(byId(nodes,'trace-receipt:w')); assert.ok(byId(nodes,'terminal-receipt:w'));
  assert.ok(!nodes.some(n => n.nodes?.includes('Repo  2m')));
  byId(nodes,'receipt:w').attributes.on_hover(false,cx);
  byId(nodes,'receipt-select-receipt:w').attributes.on_click({},cx);
  nodes = all(renderRail(view));
  assert.ok(byId(nodes,'trace-receipt:w')); assert.ok(nodes.some(n => n.nodes?.includes('Placed here')));
  assert.ok(byId(nodes,'receipt-link-receipt:w-1'));
  assert.ok(!nodes.some(n => n.attributes?.spin),'reduced motion does not animate');
});
test('an open trace gets a date separator whenever the day changes',() => {
  const day = new Date(2020,0,1,23,30).getTime(), next = new Date(2020,0,2,7,15).getTime();
  const row = {key:'receipt:w',kind:'receipt',title:'Build',text:'',state:'done',ts_ms:day,space:'Repo',duration:'2m',links:[],
    trace:[{ts_ms:day,text:'Started'},{ts_ms:day+60000,text:'Placed'},{ts_ms:next,text:'Closed'}]};
  const {view} = fixture({rows:[row]});
  view.expandedReceipts.add('receipt:w');
  const texts = all(renderRail(view)).flatMap(n => n.nodes || []).filter(x => typeof x === 'string');
  assert.deepEqual(texts.filter(x => /^\w{3} Jan \d, 2020$/.test(x)),['Wed Jan 1, 2020','Thu Jan 2, 2020']);
});
test('question clamp measures wrapped lines and expands without truncating text',() => {
  const text = 'A question that wraps over three lines at this width';
  const {view} = fixture({needsYou:[{kind:'question',questionId:'q1',text,title:'Work',ts_ms:1000,options:[]}]});
  const cx = {notify(){}};
  let nodes = all(renderRail(view));
  const content = nodes.find(n => n.nodes?.includes(text));
  assert.equal(content.attributes.line_height,'18px');
  assert.ok(nodes.some(n => n.attributes?.max_h === 54 && n.attributes?.overflow_hidden));
  content.attributes.on_size_change({height:18},cx);
  nodes = all(renderRail(view));
  assert.ok(nodes.some(n => n.nodes?.includes(text)));
  assert.equal(byId(nodes,'needs-more-text-0'),undefined);
  content.attributes.on_size_change({height:72},cx);
  nodes = all(renderRail(view));
  assert.ok(byId(nodes,'needs-more-text-0'));
  view.expandedQuestions.add('q1');
  nodes = all(renderRail(view));
  assert.ok(!nodes.some(n => n.attributes?.max_h === 54));
  assert.ok(nodes.some(n => n.nodes?.includes(text)));
});
test('off state replaces the composer with Start',() => {
  const {view} = fixture({available:false,canSend:false,status:'off'});
  const nodes = all(renderRail(view));
  assert.ok(byId(nodes,'rail-start')); assert.equal(byId(nodes,'rail-send'),undefined);
});
test('stream renders at most one window of rows',() => {
  const rows = Array.from({length:250},(_,i) => ({key:`p${i}`,kind:'person',text:`message ${i}`,ts_ms:i}));
  const {view} = fixture({rows}); view.windowStart=100;
  const nodes = all(renderRail(view));
  assert.ok(byId(nodes,'entry-p100')); assert.ok(byId(nodes,'entry-p199'));
  assert.equal(byId(nodes,'entry-p200'),undefined);
});
test('journal rows expand to the full sentence and labelled fields; system events stay hidden',() => {
  const {view} = fixture(); view.page='journal'; view.showSystem=false;
  view.filter={value:() => 'progress'};
  view.store.board={workstreams:[]};
  view.store.journal={entries:[
    {seq:1,ts_ms:1000,kind:'task.progress',actor:'delegate',ws:'w',task:'w-t1',data:{summary:'Progress update '.repeat(30)}},
    {seq:2,ts_ms:2000,kind:'watcher.started',actor:'watcher',data:{}},
  ],has_more:true};
  view.expandedJournal.add('journal:1');
  const nodes = all(renderRail(view));
  assert.ok(byId(nodes,'journal-journal:1')); assert.equal(byId(nodes,'journal-journal:2'),undefined);
  const texts = nodes.flatMap(n => n.nodes || []).filter(x => typeof x === 'string');
  assert.ok(texts.some(x => x.startsWith('Progress update Progress update')));
  assert.ok(texts.includes('Task') && texts.includes('w-t1'));
  assert.ok(texts.includes('summary'));
  assert.ok(byId(nodes,'journal-older'));
});
test('journal shows day labels and the system-events checkbox toggles the filter',() => {
  const {view} = fixture(); view.page='journal'; view.showSystem=false; view.journalStart=0;
  view.filter={value:() => ''}; view.store.board={workstreams:[]};
  const now = new Date(), today = new Date(now.getFullYear(),now.getMonth(),now.getDate(),12);
  const yesterday = new Date(today.getFullYear(),today.getMonth(),today.getDate()-1,12);
  view.store.journal={entries:[
    ...[today,yesterday].map((date,i) => ({seq:i+1,ts_ms:date.getTime(),kind:'task.progress',actor:'d',data:{summary:'Progress'}})),
    {seq:3,ts_ms:today.getTime()+1000,kind:'watcher.started',actor:'watcher',data:{}},
  ]};
  const nodes = all(renderRail(view)), checkbox = byId(nodes,'journal-system');
  assert.equal(checkbox.kind,'checkbox'); assert.equal(checkbox.attributes.checked,false);
  assert.ok(nodes.some(n => n.nodes?.includes('Today')));
  assert.ok(nodes.some(n => n.nodes?.includes('Yesterday')));
  assert.equal(byId(nodes,'journal-journal:3'),undefined);
  let notifications=0; view.journalStart=12;
  checkbox.attributes.on_change(true,{notify(){notifications++;}});
  assert.equal(view.showSystem,true); assert.equal(view.journalStart,0); assert.equal(notifications,1);
  const shown = all(renderRail(view));
  assert.equal(byId(shown,'journal-system').attributes.checked,true);
  assert.ok(byId(shown,'journal-journal:3'));
});
test('collapsed past-day summary and echo link are present',() => {
  const rows = [
    {key:'day:y',kind:'day',day:'2020-01-01',label:'Yesterday',collapsible:true,collapsed:true,countLabel:'2 asks',summary:'all done'},
    {key:'day:x',kind:'day',day:'2019-12-31',label:'Tue Dec 31, 2019',collapsible:true,collapsed:true,countLabel:'1 workstream',summary:'all done'},
    {key:'echo:1',kind:'echo',receiptKey:'receipt:w',title:'Build',text:'Finished',state:'done'},
  ];
  const {view} = fixture({rows}), nodes = all(renderRail(view));
  assert.ok(nodes.some(n => n.nodes?.includes('▸ Yesterday — 2 asks, all done')));
  assert.ok(nodes.some(n => n.nodes?.includes('▸ Tue Dec 31, 2019 — 1 workstream, all done')));
  assert.ok(byId(nodes,'day-day:y'),'past days are toggles');
  assert.ok(nodes.some(n => n.nodes?.includes('Build — Finished') && n.attributes?.truncate),'echo text stays on one line');
  assert.ok(byId(nodes,'echo-echo:1'));
});
