import {test} from 'node:test';
import assert from 'node:assert/strict';
import {registerHooks} from 'node:module';

const host = 'data:text/javascript,' + encodeURIComponent(`
export class View {}
const element = () => {
  const node = {events:{}, attrs:{}, slots:{}};
  return new Proxy(node, {get(target,key) {
    if (key in target) return target[key];
    return (...args) => {
      if (key.startsWith('on_')) target.events[key] = args.at(-1);
      else if (['trigger','content'].includes(key)) target.slots[key] = args[0];
      else target.attrs[key] = args.length <= 1 ? args[0] : args;
      return new Proxy(target,this);
    };
  }});
};
export const div = element, svg = element, h_flex = element, v_flex = element;
export const Button = {new:element}, HoverCard = {new:element};
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

test('bar renders anchored hover card for usage without giant single-line tooltip and opens details on click', () => {
  globalThis.barCalls = [];
  globalThis.barShared = {
    'herdr-kit.usage.readout.v1': {
      schema_version: 1,
      mod: 'usage-pin',
      label: 'Subscription limits, highest Antigravity Gemini Weekly 56.23% used, 4 providers. Open details',
      tooltip: 'Antigravity Gemini Weekly: 56.23% used · 4 providers. Click for details.',
      state: 'ready',
      provider: 'Antigravity',
      amount: '56.23%',
      additional: 3,
      role: 'dim',
      hover: {
        state: 'ready',
        provider: 'Antigravity',
        window: 'Gemini Weekly',
        amount: '56.23%',
        used: 0.5623,
        unlimited: false,
        credits: null,
        role: 'dim',
        stale: false,
        collecting: false,
        providerCount: 4,
        updated: '23:37',
        status: null,
        hint: '4 providers · Click for all limits',
      },
    },
  };
  const view = new KitBar();
  view.options = { usage_mod: 'usage-pin' };
  view.usageFocus = { is_focused: () => false };
  view.width = 1000;
  const hoverCard = view.usage({ chrome: '#1e1f22', popover: '#25272b', foreground: '#eeeeef', muted: '#98999b', dim: '#787a7e', border: '#33353a', accent: '#6ca0f6' });
  assert.equal(hoverCard.attrs.anchor, 'bottom_right');
  assert.ok(hoverCard.slots.trigger);
  assert.ok(hoverCard.slots.content);
  assert.equal(hoverCard.slots.trigger.attrs.tooltip, undefined);
  assert.match(hoverCard.slots.trigger.attrs.accessibility_label, /Antigravity Gemini Weekly 56\.23% used/);
  hoverCard.slots.trigger.events.on_click();
  assert.deepEqual(globalThis.barCalls, [['usage-pin.show', {}]]);
});
