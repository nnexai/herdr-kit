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
export const div=()=>globalThis.railElement('div'),svg=()=>globalThis.railElement('svg'),image=p=>{const e=globalThis.railElement('image');e.src(p);return e;};
export class View {}
export const ScrollHandle={new:()=>({})},TextareaState={new:()=>({})},InputState={new:()=>({})};
export const h_flex=()=>globalThis.railElement('h_flex'),v_flex=()=>globalThis.railElement('v_flex');
export const Button={new:id=>{const e=globalThis.railElement('button');e.id(id);return e;}},Checkbox={new:id=>{const e=globalThis.railElement('checkbox');e.id(id);return e;}},Textarea={new:()=>globalThis.railElement('textarea')},Input={new:()=>globalThis.railElement('input')},
TextView={markdown:()=>globalThis.railElement('markdown')},Popover={new:()=>globalThis.railElement('popover')};
`);
const hostUrl = 'data:text/javascript,'+encodeURIComponent(`
export const settings=()=>({dark:globalThis.railDark ?? true});
export const props=()=>({}),shared=()=>null,setShared=()=>{},run=()=>{},openOverlay=()=>'',closeOverlay=()=>{};
export const nextCommand=()=>new Promise(()=>{}),exec=()=>{throw new Error('unexpected native exec in renderer test');};
export const theme=()=>globalThis.railDark === false
  ? {foreground:'#1b1b1e',muted:'#69696b',accent:'#1f62d0',accent_foreground:'#ffffff',
     popover:'#ffffff',surface:'#ffffff',canvas:'#fafaf9',raised:'#f6f6f6',hover:'#0000000c',selected:'#00000012',border:'#00000017',input:'#00000026',done:'#2f6fe4',working:'#9a6700',blocked:'#c93c2c'}
  : {foreground:'#eeeeef',muted:'#98999b',accent:'#96b2d7',accent_foreground:'#17191d',
     popover:'#25272b',surface:'#17191d',canvas:'#131518',raised:'#2c2e31',hover:'#ffffff0e',selected:'#ffffff17',border:'#ffffff18',input:'#ffffff24',done:'#76a8ff',working:'#e5c07b',blocked:'#f0705f'};
`);
registerHooks({resolve(specifier,context,next) {
  const url = ['gpui-kit','gpui-base'].includes(specifier) ? nativeUrl : specifier === 'herdr' ? hostUrl : null;
  return url ? {url,shortCircuit:true} : next(specifier,context);
}});
const {renderRail, palette} = await import('../mods/kit-coord-ui/render.js');
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
  assert.equal(content.attributes.line_height,'20px');
  assert.ok(nodes.some(n => n.attributes?.max_h === 60 && n.attributes?.overflow_hidden));
  content.attributes.on_size_change({height:20},cx);
  nodes = all(renderRail(view));
  assert.ok(nodes.some(n => n.nodes?.includes(text)));
  assert.equal(byId(nodes,'needs-more-text-0'),undefined);
  content.attributes.on_size_change({height:80},cx);
  nodes = all(renderRail(view));
  assert.ok(byId(nodes,'needs-more-text-0'));
  view.expandedQuestions.add('q1');
  nodes = all(renderRail(view));
  assert.ok(!nodes.some(n => n.attributes?.max_h === 60));
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

test('roles menu item opens Roles list with fixed hover slot, no bottom buttons, and consistent header/margins',() => {
  const roles = [
    {name:'agy-default',source:'builtin',harness:'agy',model:'gemini-4-neon-high',effort:null,session:'agy-conversation',start:'agy -i {brief}',resume:'agy --conversation {session_id} -i {brief}',resumable:true,steering:false,body:'Antigravity delegate.'},
    {name:'claude-opus',source:'override',harness:'claude',model:'opus',effort:'high',session:'preset',start:'claude --session-id {session_id} {brief}',resume:'claude --resume {session_id} {brief}',resumable:true,steering:false,body:'Opus override.'},
    {name:'my-custom',source:'file',harness:'omp',model:'@advisor',effort:null,session:'dir',start:'omp --session-dir {session_dir} {brief}',resume:null,resumable:false,steering:true,body:'Custom reviewer.'},
  ];
  const makeInput = (initial = '') => { let v = initial; return {value:() => v,set_value(x){v=x;},focus(){}}; };
  const {view} = fixture();
  view.store.roles = roles;
  view.roleFilter = makeInput('');
  view.rolesMode = 'list';
  view.hoveredRole = null;
  view.confirmRoleAction = null;
  view.selectedRole = null;
  view.roleFrom = '';
  view.fromMenuOpen = false;
  view.roleFields = {
    name:makeInput(''),harness:makeInput(''),model:makeInput(''),effort:makeInput(''),
    session:makeInput(''),start:makeInput(''),resume:makeInput(''),trust:makeInput(''),body:makeInput(''),
  };
  view.roleByName = name => roles.find(r => r.name === name) || null;
  view.openRoles = cx => { view.page = 'roles'; view.rolesMode = 'list'; cx.notify(); };
  view.openRoleDetail = (name,cx) => { view.rolesMode = 'detail'; view.selectedRole = name; cx.notify(); };
  view.openRoleEdit = (name,cx) => { view.rolesMode = 'edit'; view.selectedRole = name; cx.notify(); };
  view.openRoleCreate = (from,cx) => { view.rolesMode = 'create'; view.roleFrom = from || ''; cx.notify(); };
  view.confirmRemoveRole = (name,action,cx) => { view.confirmRoleAction = {name,action}; cx.notify(); };
  view.rolesBack = cx => { view.rolesMode = 'list'; cx.notify(); };
  view.saveRole = () => {};
  view.applyRoleTemplate = () => {};
  view.pickRoleHarness = () => {};
  view.runRemoveRole = () => {};

  const cx = {notify(){},stop_propagation(){}};
  // 1. Stream header and menu item for Roles
  const streamRoot = renderRail(view);
  const streamHeader = streamRoot.nodes[0];
  assert.ok(byId(all(streamRoot),'rail-roles'),'Roles menu item exists in header menu');

  // 2. Switch to Roles list: header geometry matches timeline
  view.page = 'roles';
  const rolesRoot = renderRail(view);
  const rolesHeader = rolesRoot.nodes[0];
  assert.equal(rolesHeader.attributes.h, streamHeader.attributes.h);
  assert.equal(rolesHeader.attributes.px, streamHeader.attributes.px);
  const listNodes = all(rolesRoot);
  assert.ok(byId(listNodes,'roles-new'),'+ New role is in the top bar');
  assert.ok(byId(listNodes,'role-row-agy-default'));
  assert.ok(byId(listNodes,'role-row-claude-opus'));
  assert.ok(byId(listNodes,'role-row-my-custom'));
  // Idle: source labels shown, hover buttons hidden
  const idleTexts = listNodes.flatMap(n => n.nodes || []).filter(x => typeof x === 'string');
  assert.ok(idleTexts.includes('builtin') && idleTexts.includes('override') && idleTexts.includes('file'));
  assert.equal(byId(listNodes,'role-edit-my-custom'),undefined);

  // Hover on custom file role: shows Edit, Copy, Delete inside fixed 188x24 slot without shifting layout
  byId(listNodes,'role-row-my-custom').attributes.on_hover(true,cx);
  let hoverNodes = all(renderRail(view));
  assert.ok(byId(hoverNodes,'role-edit-my-custom'));
  assert.ok(byId(hoverNodes,'role-copy-my-custom'));
  assert.ok(byId(hoverNodes,'role-delete-my-custom'));
  assert.equal(byId(hoverNodes,'role-reset-my-custom'),undefined);

  // Hover on built-in override: shows Edit, Copy, Reset (not Delete)
  byId(hoverNodes,'role-row-claude-opus').attributes.on_hover(true,cx);
  hoverNodes = all(renderRail(view));
  assert.ok(byId(hoverNodes,'role-edit-claude-opus'));
  assert.ok(byId(hoverNodes,'role-copy-claude-opus'));
  assert.ok(byId(hoverNodes,'role-reset-claude-opus'));
  assert.equal(byId(hoverNodes,'role-delete-claude-opus'),undefined);

  // Hover on pure built-in: shows Edit, Copy (no Delete or Reset)
  byId(hoverNodes,'role-row-agy-default').attributes.on_hover(true,cx);
  hoverNodes = all(renderRail(view));
  assert.ok(byId(hoverNodes,'role-edit-agy-default'));
  assert.ok(byId(hoverNodes,'role-copy-agy-default'));
  assert.equal(byId(hoverNodes,'role-delete-agy-default'),undefined);
  assert.equal(byId(hoverNodes,'role-reset-agy-default'),undefined);

  // 3. Detail view shows resolved fields and prompt body, with actions at top (no bottom bar)
  view.openRoleDetail('claude-opus',cx);
  const detailRoot = renderRail(view);
  const detailNodes = all(detailRoot);
  assert.ok(byId(detailNodes,'role-detail-edit'));
  assert.ok(byId(detailNodes,'role-detail-copy'));
  assert.ok(byId(detailNodes,'role-detail-reset'));
  const detailTexts = detailNodes.flatMap(n => n.nodes || []).filter(x => typeof x === 'string');
  assert.ok(detailTexts.includes('Opus override.'));
  assert.ok(detailTexts.includes('claude --session-id {session_id} {brief}'));
  // Last child of roles detail view is the scrollable detail container, never a bottom button bar
  const detailPage = detailRoot.nodes[1];
  assert.equal(detailPage.nodes.at(-1).attributes.id,'roles-detail');

  // 4. Create view supports blank and copy-from templates, Save/Cancel at top
  view.openRoleCreate('agy-default',cx);
  view.roleFields.name.set_value('agy-fast');
  const createRoot = renderRail(view);
  const createNodes = all(createRoot);
  assert.ok(byId(createNodes,'role-from-blank'));
  assert.ok(byId(createNodes,'role-from-trigger'));
  assert.ok(byId(createNodes,'role-form-save'));
  assert.ok(byId(createNodes,'role-form-cancel'));
  assert.equal(createRoot.nodes[1].nodes.at(-1).attributes.id,'roles-form');

  // 5. Edit view on built-in notes override and keeps Save/Cancel at top
  view.openRoleEdit('agy-default',cx);
  const editRoot = renderRail(view);
  const editTexts = all(editRoot).flatMap(n => n.nodes || []).filter(x => typeof x === 'string');
  assert.ok(editTexts.includes('Saves as override'));
  assert.equal(editRoot.nodes[1].nodes.at(-1).attributes.id,'roles-form');
});

test('readability refinements: dark/light contrast, transparent ghost actions, narrow adaptation and focus cues',() => {
  const lum = hex => {
    const ch = i => {
      const v = parseInt(hex.slice(i,i+2),16)/255;
      return v <= 0.03928 ? v/12.92 : ((v+0.055)/1.055)**2.4;
    };
    return 0.2126*ch(1) + 0.7152*ch(3) + 0.0722*ch(5);
  };
  const contrast = (a,b) => {
    const la = lum(a), lb = lum(b);
    return (Math.max(la,lb)+0.05)/(Math.min(la,lb)+0.05);
  };

  // Dark palette: 3-tier ink and WCAG AA contrast on popover & cards
  globalThis.railDark = true;
  const dark = palette();
  assert.ok(contrast(dark.secondary, dark.popover) >= 7.0, 'secondary text meets high contrast on dark popover');
  assert.ok(contrast(dark.muted, dark.popover) >= 5.5, 'muted text meets AA on dark popover');
  assert.ok(contrast(dark.blocked, '#33292d') >= 4.5, 'blocked label meets AA on needs-you tint');
  assert.notEqual(dark.input, dark.popover, 'dark input surface separates from dialog popover');

  // Light palette: fixes translucent black input bug (#00000026) and preserves AA ink
  globalThis.railDark = false;
  const light = palette();
  assert.equal(light.input, '#ffffff', 'light input uses clean surface instead of muddy alpha overlay');
  assert.ok(contrast(light.secondary, light.popover) >= 7.0, 'secondary text meets high contrast on light popover');
  assert.ok(contrast(light.muted, light.popover) >= 5.5, 'muted text meets AA on light popover');
  globalThis.railDark = true;

  // Ghost actions inside hovered/tinted rows stay transparent (no opaque popover cutout)
  const row = {key:'receipt:w',kind:'receipt',title:'Build',text:'Working',state:'running',ts_ms:1000,space:'Repo',duration:'2m',
    trace:[],links:[{kind:'terminal',target:'t'}]};
  const {view} = fixture({rows:[row],needsYou:[{kind:'question',questionId:'q1',text:'Long text',title:'Work',ts_ms:1000,options:['Yes']}]});
  view.hoveredReceipt = 'receipt:w';
  view.clampedQuestions.add('q1');
  view.inputFocused = true;
  const stdNodes = all(renderRail(view));
  assert.equal(byId(stdNodes,'terminal-receipt:w').attributes.bg, '#00000000');
  assert.equal(byId(stdNodes,'needs-more-text-0').attributes.bg, '#00000000');
  assert.ok(stdNodes.some(n => n.attributes?.border_color === dark.accent && n.attributes?.bg === dark.input), 'focused composer highlights border');

  // Narrow width (< 460px): adapts horizontal margins and compact fixed slot while keeping Stream/Roles headers aligned
  view.width = 380;
  const narrowStream = renderRail(view);
  assert.equal(narrowStream.nodes[0].attributes.px, 12);
  const narrowNodes = all(narrowStream);
  assert.ok(narrowNodes.some(n => n.attributes?.w === 132 && n.attributes?.flex_shrink_0), 'narrow receipt slot stays fixed at 132px');
  assert.ok(byId(narrowNodes,'terminal-receipt:w').nodes.includes('Terminal'));

  view.page = 'roles';
  view.rolesMode = 'create';
  view.store.roles = [];
  const makeInput = () => ({value:() => '',set_value(){},focus(){}});
  view.roleFilter = makeInput();
  view.roleFields = {name:makeInput(),harness:makeInput(),model:makeInput(),effort:makeInput(),session:makeInput(),start:makeInput(),resume:makeInput(),trust:makeInput(),body:makeInput()};
  const narrowRoles = renderRail(view);
  assert.equal(narrowRoles.nodes[0].attributes.h, narrowStream.nodes[0].attributes.h);
  assert.equal(narrowRoles.nodes[0].attributes.px, narrowStream.nodes[0].attributes.px);
});

test('attachments render in collapsed receipt cue, open trace evidence with inline image preview, HTML confirm, missing/ref states, journal and plain-text viewer',() => {
  const imgAtt = {id:'ws-abc234-t1-a1',ws:'ws-abc234',task:'ws-abc234-t1',task_title:'Polish',label:'card-dark.png',kind:'image',mode:'staged',bytes:231424,ts_ms:1000,report_kind:'progress',availability:'available',resolved_path:'/tmp/a1.png'};
  const htmlAtt = {id:'ws-abc234-t1-a2',ws:'ws-abc234',task:'ws-abc234-t1',task_title:'Polish',label:'mock.html',kind:'html',mode:'staged',bytes:49152,ts_ms:2000,report_kind:'done',availability:'available',resolved_path:'/tmp/a2.html'};
  const refAtt = {id:'ws-abc234-t1-a3',ws:'ws-abc234',task:'ws-abc234-t1',task_title:'Polish',label:'trace.log',kind:'log',mode:'ref',reason:'exceeds_stage_limit',bytes:26004684,ts_ms:2000,report_kind:'done',availability:'available',resolved_path:'/tmp/trace.log'};
  const missAtt = {id:'ws-abc234-t1-a4',ws:'ws-abc234',task:'ws-abc234-t1',task_title:'Polish',label:'gone.png',kind:'image',mode:'ref',source_path:'/tmp/wt/gone.png',ts_ms:1500,report_kind:'progress',availability:'missing'};
  const ws = {id:'ws-abc234',title:'Polish UI',state:'cleaned',tasks:[{id:'ws-abc234-t1',title:'Polish',attachments:[imgAtt,htmlAtt,refAtt,missAtt]}]};
  const row = {key:'receipt:ws-abc234',kind:'receipt',ws:'ws-abc234',wsState:'cleaned',title:'Polish UI',text:'Done',state:'done',ts_ms:1000,space:'herdr-kit',duration:'14m',
    links:[],attachmentCount:4,attachments:[imgAtt,htmlAtt,refAtt,missAtt],
    trace:[{ts_ms:1000,text:'Progress: Dark mode',attachments:[imgAtt]}],source:ws};
  const {view} = fixture({rows:[row],needsYou:[{kind:'attention',title:'Polish UI',text:'Contrast check failed',ts_ms:2000,attachment:refAtt}]});
  view.store.imagePreviews = new Map();
  view.store.wsArtifactMap = () => new Map([imgAtt,htmlAtt,refAtt,missAtt].map(a => [a.id,a]));
  view.expandedEvidence = new Set();
  const cx = {notify(){},stop_propagation(){}};

  // Collapsed receipt preserves 2-line layout with quiet file count in fixed slot, and never renders inline image
  let nodes = all(renderRail(view));
  assert.ok(nodes.some(n => n.nodes?.includes('herdr-kit · 4 files  14m')));
  assert.ok(byId(nodes,'needs-att-view-0'),'Needs You card shows compact attachment cue');
  assert.equal(nodes.some(n => n.kind === 'image'),false,'collapsed receipts never render images');

  // Expanded receipt shows Evidence roll-up, missing/ref cues, HTML inline confirm, and inline image preview
  view.expandedReceipts.add('receipt:ws-abc234');
  view.confirmHtmlId = 'trace-ws-abc234:ws-abc234-t1-a2';
  view.store.imagePreviews.set(imgAtt.id,{status:'ready',relPath:'data/kit-coord-ui/preview/p-1-test.png',width:320,height:200,format:'png',bytes:231424});
  nodes = all(renderRail(view));
  const texts = nodes.flatMap(n => n.nodes || []).filter(x => typeof x === 'string');
  assert.ok(texts.includes('Evidence · 4'));
  assert.ok(texts.includes('worktree cleaned · 2 staged'));
  assert.ok(texts.some(t => t.includes('Attached: card-dark.png')));
  assert.ok(texts.some(t => t.includes('Not staged (>10 MB limit)')));
  assert.ok(texts.some(t => t.includes('Missing on disk')));
  assert.ok(texts.some(t => t.includes('Opens local HTML in your browser; scripts can run.')));
  const imgNode = nodes.find(n => n.kind === 'image');
  assert.ok(imgNode,'renders native GPUI image element inside open preview');
  assert.equal(imgNode.attributes.src,'data/kit-coord-ui/preview/p-1-test.png');

  // Narrow width (< 460px): collapsed receipt keeps count + duration inside 132px, selected row reveals wrapped actions
  view.width = 380;
  view.expandedReceipts.clear();
  nodes = all(renderRail(view));
  assert.ok(nodes.some(n => n.nodes?.includes('4 files  14m')));
  view.expandedReceipts.add('receipt:ws-abc234');
  view.selectedAttachment = 'trace-ws-abc234:ws-abc234-t1-a1';
  nodes = all(renderRail(view));
  assert.ok(byId(nodes,'att-n-preview-trace-ws-abc234:ws-abc234-t1-a1'));
  assert.ok(byId(nodes,'att-n-copy-trace-ws-abc234:ws-abc234-t1-a1'));

  // Plain-text viewer subpage has top actions only (no bottom bar) and inert text lines
  view.width = 680;
  view.page = 'artifact';
  view.store.artifactViewer = {attachment:htmlAtt,loading:false,text:'<html>\n<script>alert(1)</script>\n</html>',truncated:false,total_bytes:41,shown_bytes:41,cap_bytes:65536,error:''};
  const viewerRoot = renderRail(view);
  const viewerNodes = all(viewerRoot);
  assert.ok(byId(viewerNodes,'artifact-back'));
  assert.ok(byId(viewerNodes,'artifact-viewer-browser'));
  assert.ok(byId(viewerNodes,'artifact-viewer-copy'));
  assert.equal(viewerRoot.nodes[1].nodes.at(-1).attributes.id,'artifact-viewer-body','no bottom action bar in viewer');
  assert.ok(viewerNodes.some(n => n.nodes?.includes('<script>alert(1)</script>')));
});
