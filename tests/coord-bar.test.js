import {test} from 'node:test';
import assert from 'node:assert/strict';
import {registerHooks} from 'node:module';

const host = 'data:text/javascript,' + encodeURIComponent(`
export class View {}
const element = () => {
  const node = {events:{}};
  return new Proxy(node, {get(target,key) {
    if (key in target) return target[key];
    return (...args) => { if (key.startsWith('on_')) target.events[key] = args.at(-1); return new Proxy(target,this); };
  }});
};
export const div = element, svg = element, h_flex = element;
export const Button = {new:element};
export const agents = () => [], spaces = () => [], theme = () => ({}), props = () => ({});
export const shared = key => globalThis.barShared[key];
export const run = (name,args) => globalThis.barCalls.push([name,args]);
export const exec = () => {};
`);
registerHooks({resolve(specifier, context, next) {
  return ['gpui-kit','gpui-base','herdr'].includes(specifier) ? {url:host,shortCircuit:true} : next(specifier,context);
}});
const {default:KitBar} = await import('../mods/herdr-kit-bar/main.js');

function button(state, click, ui = 'coord-ui-pin') {
  globalThis.barCalls = [];
  globalThis.barShared = {
    'herdr-kit.coord.readout.v1': {schema_version:1, mod:'coord-pin', state, click, tooltip:'Click to focus the coordinator.', tier:'icon'},
    'herdr-kit.coord.status.v1': {coordinator:{terminal:'coord-terminal'}},
  };
  const view = new KitBar();
  view.options = {coord_mod:'coord-pin',coord_ui_mod:ui};
  view.coordFocus = {is_focused:()=>false};
  return view.coord({});
}

test('bar opens the pinned dialog and falls back to terminal focus', () => {
  let control = button('running','focus');
  control.events.on_click();
  assert.deepEqual(globalThis.barCalls, [['coord-ui-pin.show',{}]]);
  let stopped = false;
  control.events.on_mouse_down({}, {stop_propagation(){stopped=true;}});
  assert.equal(stopped,true);
  assert.deepEqual(globalThis.barCalls.at(-1), ['coord-ui-pin.show',{}]);
  control = button('running','focus',null);
  control.events.on_click();
  assert.deepEqual(globalThis.barCalls, [['agent.focus',{terminal:'coord-terminal'}]]);
});

test('bar starts, repairs and rereads without opening the dialog', () => {
  for (const [state, click, command] of [['not_running','up','start-coordinator'],['degraded','up','start-coordinator'],['unavailable','reread','reread-status']]) {
    button(state,click).events.on_click();
    assert.deepEqual(globalThis.barCalls, [[`coord-pin.${command}`,{}]]);
  }
  button('starting','none').events.on_click();
  assert.deepEqual(globalThis.barCalls, []);
});
