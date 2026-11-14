import { View, div } from 'gpui-kit';
import { ScrollHandle, InputState } from 'gpui-base';
import { agents, props, openOverlay, closeOverlay, nextCommand, run, setShared } from 'herdr';
import { MOD_NAME } from './runtime.js';
import { store } from './store.js';
import { boardModel, timelineModel, timelineWindow, moveSelection, reconcileSelection, actionCommand, statusbarModel } from './model.js';
import { renderStatus } from './ui-status.js';
import { renderBoard } from './ui-board.js';
import { renderTimeline } from './ui-timeline.js';

let strips = 0, dialog = null, commandRevision = 0;
export default class Coordinator extends View {
  init(_p, cx) {
    const p = props() || {};
    this.details = p.view === 'board' || (!p.view && !!p.overlay);
    this.source = p.view === 'source';
    this.closed = false; this.width = 280; this.store = store;
    this.focus = cx.focus_handle(); this.scroll = ScrollHandle.new();
    this.ui = { view: p.tab === 'timeline' ? 'timeline' : 'board', selectedKey: null,
      expanded: new Set(), collapsed: new Set(), showClosed: new Set(), scope: {}, text: '', detail: 'key', now: Date.now(), liveAgents: new Map() };
    this.filter = InputState.new({placeholder:'Filter id, kind, text', value:''});
    this.filter.on('change', (value, cx) => { this.ui.text = this.filter.value(); cx.notify(); });
    this.filter.on('submit', (_value, cx) => { this.focus.focus(); cx.notify(); });
    store.init();
    if (this.details) {
      store.overlay = this;
      store.setScope({});
      this.overlayId = p.overlay;
      if (p.overlay) dialog = p.overlay;
      this.focus.focus();
      store.refresh(cx, 'status', true); store.refresh(cx, this.ui.view === 'board' ? 'board' : 'log', true);
      cx.timer.every(10000, cx => { if (!this.closed) { store.refresh(cx, 'status'); store.refresh(cx, this.ui.view === 'board' ? 'board' : 'log', false, {mode:'prepend'}); } });
      cx.timer.every(30000, cx => cx.notify());
      store.watch(cx, this);
    } else {
      this.stripOwner = strips++ === 0;
      if (this.stripOwner) {
        store.refresh(cx, 'status', true); store.watch(cx, this);
        cx.timer.every(20000, cx => { if (!this.closed) store.refresh(cx, 'status'); });
        cx.timer.every(60000, cx => cx.notify());
        cx.spawn(async cx => {
          while (!this.closed) {
            const command = await nextCommand();
            if (this.closed) break;
            if (command === 'show') this.openBoard('board');
            else if (command === 'start-coordinator') store.up(cx);
            else if (command === 'reread-status') store.refresh(cx, 'status', true);
            else if (command === 'clear-context') store.restart(cx);
            else if (command === 'restart-now') store.restart(cx, true);
            else if (command === 'restart') {
              this.openBoard('board');
              if (!store.restartPending) store.restart(cx);
            } else if (command === 'timeline') {
              if (store.overlay && !store.overlay.closed) store.overlay.key({key:'t'}, cx, true);
              else this.openBoard('timeline');
            } else if (command === 'focus') this.focus.focus();
            else if (store.overlay && !store.overlay.closed) {
              const keys = {up:'up',down:'down',home:'home',end:'end',
                'page-up':'pageup','page-down':'pagedown',right:'right',left:'left',
                space:'space',enter:'enter',board:'b',refresh:'r',all:'a',log:'l',
                peek:'p',scope:'s',copy:'c','clear-scope':'x',filter:'/'};
              const key = keys[command];
              if (key) store.overlay.key({key}, cx, true);
            }
            setShared('herdr-kit.coord.command', ++commandRevision);
          }
        });
      }
    }
  }
  close() { this.closed = true; if (this.details) { if (store.overlay === this) store.overlay = null; if (dialog === props()?.overlay) dialog = null; } else strips = Math.max(0, strips - 1); }
  openBoard(tab = 'board') { if (!dialog) dialog = openOverlay({mod: MOD_NAME, props:{view:'board',tab}, kind:'dialog',width:760,height:600}); }
  notify(cx) { cx.notify(); }
  start(cx) { store.up(cx); }
  clearContext(cx) { if (store.restartPending) { this.confirmRestart = true; this.focus.focus(); cx.notify(); } else store.restart(cx); }
  refresh(cx) { store.refresh(cx, 'status', true); if (this.details) store.refresh(cx, this.ui.view === 'board' ? 'board' : 'log', true, this.ui.view === 'timeline' && store.log ? {mode:'prepend'} : {}); }
  setTab(tab, cx) { if (tab === this.ui.view) return; this.ui.view = tab; this.ui.selectedKey = null; this.selectionIndex = 0; this.focus.focus(); store.refresh(cx, tab === 'board' ? 'board' : 'log', true, tab === 'timeline' && store.log ? {mode:'prepend'} : {}); cx.notify(); }
  select(row, cx) { this.ui.selectedKey = row.key; this.focus.focus(); cx.notify(); }
  acceptNew(cx) {
    store.acceptNew();
    const rows = timelineModel(store.log, this.ui).rows;
    this.ui.selectedKey = rows.find(row => row.selectable)?.key ?? null;
    this.selectionIndex = 0;
    this.scroll.scroll_to_item(0);
    this.focus.focus();
    cx.notify();
  }
  navigate(action, row, cx) {
    const effect = actionCommand(action, row, store.status);
    if (!effect) return;
    if (effect.kind === 'run') { closeOverlay(this.overlayId); run(effect.name, effect.args); if (effect.then) run(effect.then.name, effect.then.args); }
    else if (effect.kind === 'up') store.up(cx);
    else if (effect.kind === 'log') { this.ui.scope = effect.scope || {}; store.setScope(this.ui.scope); this.setTab('timeline', cx); }
    else if (effect.kind === 'copy') cx.write_to_clipboard(effect.text);
    else if (effect.kind === 'toggle') {
      if (row.kind === 'closed') {
        const id = row.source.id; if (this.ui.showClosed.has(id)) this.ui.showClosed.delete(id); else this.ui.showClosed.add(id);
      } else {
        const id = row.key; if (row.collapsed) { this.ui.collapsed.delete(id); this.ui.collapsed.add('open:' + id); }
        else { this.ui.collapsed.delete('open:' + id); this.ui.collapsed.add(id); }
      }
      cx.notify();
    }
  }
  key(e, cx, command = false) {
    if (e.modifiers?.control || e.modifiers?.alt || e.modifiers?.platform) return;
    const key = e.key; this.ui.now = Date.now();
    if (this.confirmRestart && store.restartPending) {
      if (key === 'enter') { this.confirmRestart = false; store.restart(cx, true); }
      else if (key === 'escape') this.confirmRestart = false;
      else return;
      if (!command) cx.stop_propagation(); cx.notify(); return;
    }
    const model = this.ui.view === 'board' ? boardModel(store.board, store.status, this.ui) : timelineModel(store.log, this.ui);
    const rows = model?.rows || [], row = rows.find(r => r.key === this.ui.selectedKey);
    if (key === 'home' && this.ui.view === 'timeline' && store.newCount) {
      this.acceptNew(cx);
    } else if (['up','down','home','end','pageup','pagedown'].includes(key)) {
      this.ui.selectedKey = moveSelection(rows, this.ui.selectedKey, key, 8);
      let index = this.scrollIndices?.get(this.ui.selectedKey);
      if (this.ui.view === 'timeline') {
        const window = timelineWindow(rows, this.ui.selectedKey, this.timelineStart || 0);
        const rowIndex = rows.findIndex(row => row.key === this.ui.selectedKey);
        if (rowIndex >= 0) index = rowIndex - window.start + (window.newer ? 1 : 0) + (store.logFailed || store.failed ? 1 : 0);
      }
      if (index != null) this.scroll.scroll_to_item(index);
    } else if (key === 't') this.setTab('timeline', cx);
    else if (key === 'b') this.setTab('board', cx);
    else if (key === 'r') this.refresh(cx);
    else if (key === 'n') this.clearContext(cx);
    else if (key === '/' && this.ui.view === 'timeline') this.filter.focus();
    else if (key === 'a' && this.ui.view === 'timeline') this.ui.detail = this.ui.detail === 'key' ? 'all' : 'key';
    else if ((key === 'x' || key === 'backspace') && this.ui.view === 'timeline') { this.ui.scope = {}; store.setScope({}); store.refresh(cx,'log',true); }
    else if (row && key === 'enter' && row.kind === 'more') store.refresh(cx,'log',true,{mode:'append'});
    else if (row && key === 'space' && row.kind === 'closed') this.navigate('toggle', row, cx);
    else if (row && (key === 'right' || key === 'space' || (key === 'enter' && this.ui.view === 'timeline'))) { if (this.ui.expanded.has(row.key)) this.ui.expanded.delete(row.key); else this.ui.expanded.add(row.key); }
    else if (row && key === 'left') { this.ui.expanded.delete(row.key); if (row.kind === 'workstream' && !row.collapsed) this.navigate('toggle', row, cx); }
    else if (row && key === 'enter') this.navigate(row.kind === 'workstream' || row.kind === 'closed' ? 'toggle' : row.actions?.[0]?.id || 'show', row, cx);
    else if (row && key === 'p') this.navigate('peek', row, cx);
    else if (row && key === 'l') this.navigate('log', row, cx);
    else if (row && key === 's' && this.ui.view === 'timeline') { const entry = row.source; if (entry?.task || entry?.ws) { this.ui.scope = entry.task ? {task:entry.task} : {ws:entry.ws}; store.setScope(this.ui.scope); store.refresh(cx,'log',true); } }
    else if (row && key === 's') this.navigate('show-space', row, cx);
    else if (row && key === 'c') cx.write_to_clipboard(this.ui.view === 'timeline' ? `${row.source?.seq ?? ''} ${row.text || ''}` : String(row.key));
    else return;
    if (!command) cx.stop_propagation(); cx.notify();
  }
  reconcile() {
    if (!this.details) return;
    const rows = (this.ui.view === 'board' ? boardModel(store.board, store.status, this.ui) : timelineModel(store.log, this.ui)).rows;
    this.ui.selectedKey = reconcileSelection(rows, this.ui.selectedKey, this.selectionIndex || 0);
    const index = rows.findIndex(row => row.key === this.ui.selectedKey);
    if (index >= 0) this.selectionIndex = index;
  }
  render() {
    if (!this.details) {
      const vm = statusbarModel(store.status, {width:80, now:Date.now(), starting:store.starting, failed:store.failed, failures:store.failures, lastOkMs:store.status?.generated_at_ms, error:store.error});
      const readout = {schema_version:1, mod:MOD_NAME, ...vm, icon_path:`mods/${MOD_NAME}/${vm.icon}`};
      const encoded = JSON.stringify(readout);
      if (encoded !== this.published) { this.published = encoded; setShared('herdr-kit.coord.readout.v1',readout); }
    }
    if (this.source) return div().w(0).h(0);
    if (!store.restartPending || store.status?.state !== 'running') this.confirmRestart = false;
    this.ui.now = Date.now(); this.ui.liveAgents = new Map(agents().map(a => [a.address,a.status]));
    this.reconcile();
    return div().id('coord-root').size_full().on_size_change((size,cx) => { if (this.width !== size.width) { this.width = size.width; cx.notify(); } }).child(this.details ? (this.ui.view === 'timeline' ? renderTimeline(this) : renderBoard(this)) : renderStatus(this));
  }
}
