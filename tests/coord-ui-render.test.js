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
export const ScrollHandle={new:()=>({})},TextareaState={new:()=>({})};
export const h_flex=()=>globalThis.railElement('h_flex'),v_flex=()=>globalThis.railElement('v_flex');
export const Button={new:id=>{const e=globalThis.railElement('button');e.id(id);return e;}},Textarea={new:()=>globalThis.railElement('textarea')},
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
const {default:MissionLog} = await import('../mods/kit-coord-ui/main.js');
function all(node) {return typeof node === 'object' && node ? [node,...node.nodes.flatMap(all)] : [];}
test('an off-screen question remains reachable without invalidating the native log',() => {
  const question = {key:'question:1',kind:'question',label:'Question',ts_ms:1000,text:'Queue the install?',questionId:'q1',options:[],links:[]};
  const vm = {rows:[question],questions:[question],available:true,canSend:true,working:false,head:'Coordinator idle · listening',step:'One question is waiting',elapsed:'now',status:'running',active:2,sync:'09:36'};
  const view = {width:380,reducedMotion:true,store:{status:{coordinator:{}},notice:''},rowHeights:new Map(),windowStart:0,
    rootFocus:{},logFocus:{},menuFocus:{},scroll:{},input:{},draft:'',question:null,model:()=>vm,key(){},questionOffscreen:()=>true};
  const root = renderRail(view), nodes = all(root);
  const pill = nodes.find(node => node.attributes.id === 'rail-question-jump');
  assert.ok(pill,'the pending question is reachable while its row is off-screen');
  assert.ok(pill.nodes.includes('↑ 1 question waiting'));
  const wrapper = nodes.find(node => node.nodes.some(child => child?.attributes?.id === 'rail-log'));
  const floating = wrapper.nodes.find(node => node.attributes.absolute);
  assert.ok(floating,'the question affordance must not consume vertical log space');
  assert.ok(all(floating).includes(pill));
});

test('jumping to a measured question hides its pill until it scrolls above the log again',() => {
  const question = {key:'q1'}, reply = {key:'a1'};
  const vm = {rows:[question,reply],questions:[question]};
  const view = Object.assign(Object.create(MissionLog.prototype),{
    rowHeights:new Map([['q1',100],['a1',500]]),logHeight:300,scrollTop:0,windowStart:0,follow:true,
    model:()=>vm,scroll:{scroll_to_item(){}},logFocus:{focus(){}},
  });
  const cx = {notify(){}};
  assert.equal(view.questionOffscreen(vm),true);
  view.jumpQuestion(cx);
  assert.equal(view.questionOffscreen(vm),false);
  view.scrolled(-110,cx);
  assert.equal(view.questionOffscreen(vm),true);
  view.scrolled(100,cx);
  assert.equal(view.questionOffscreen(vm),false);
});
