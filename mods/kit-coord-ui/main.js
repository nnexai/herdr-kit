import { View, div } from 'gpui-kit';
import { ScrollHandle, TextareaState, InputState } from 'gpui-base';
import { props, shared, setShared, run, openOverlay, closeOverlay, nextCommand } from 'herdr';
import { MOD_NAME, DEPENDENCY_MOD } from './runtime.js';
import { railModel, focusAction, clock } from './model.js';
import { RailStore } from './store.js';
import { renderRail } from './render.js';
import { systemReducedMotion } from './motion.js';

const MOTION_KEY = 'herdr-kit.coord-ui.reduced-motion';
let dialog = null;
export default class CoordinatorDialog extends View {
  init(_props,cx) {
    const p = props() || {};
    this.details = p.view === 'log' || (!p.view && !!p.overlay);
    this.closed = false; this.width = this.details ? 680 : 240;
    this.overlayId = p.overlay; this.rootFocus = cx.focus_handle(); this.menuFocus = cx.focus_handle();
    this.menuOpen = false; this.confirmRestart = false;
    if (!this.details) {
      // Invisible mount: receives the `show` command the status bar sends.
      cx.spawn(async cx => { while (!this.closed) { const command = await nextCommand(); if (this.closed) break; if (command === 'show') this.open(); } });
      return;
    }
    if (p.overlay) dialog = p.overlay;
    this.store = new RailStore(); this.scroll = ScrollHandle.new(); this.logFocus = cx.focus_handle();
    this.page = 'stream'; this.expandedDays = new Set(); this.expandedReceipts = new Set();
    this.expandedQuestions = new Set(); this.clampedQuestions = new Set(); this.expandedJournal = new Set(); this.needsAll = false;
    this.hoveredReceipt = null;
    this.showSystem = false; this.filter = InputState.new({placeholder:'Filter journal',value:''});
    this.filter.on('change',(_event,cx) => cx.notify());
    this.idleInput = TextareaState.new({placeholder:'Tell the coordinator…',value:''});
    this.busyInput = TextareaState.new({placeholder:'Coordinator working…',value:''});
    this.input = this.idleInput;
    for (const input of [this.idleInput,this.busyInput]) {
      input.set_auto_grow(1,6);
      input.on('change',(_event,cx) => { if (input === this.input) this.draft = input.value(); cx.notify(); });
      input.on('focus',() => { this.inputFocused = true; });
      input.on('blur',() => { this.inputFocused = false; });
    }
    this.rowHeights = new Map(); this.logHeight = 0; this.scrollTop = 0; this.lastAvailable = null;
    this.draft = ''; this.question = null; this.follow = true; this.windowStart = 0; this.lastRows = '';
    this.reducedMotion = p.reducedMotion ?? !!shared(MOTION_KEY);
    if (p.reducedMotion == null && shared(MOTION_KEY) == null) cx.spawn(async cx => {
      const reduced = await systemReducedMotion();
      if (!this.closed && shared(MOTION_KEY) == null) { this.reducedMotion = reduced; cx.notify(); }
    });
    this.input.focus(); this.store.refresh(cx,this);
    cx.spawn(async cx => { await cx.sleep(50); if (!this.closed) this.input.focus(); });
    // A timer refreshes data and elapsed time, never manufactures a working turn or animation.
    cx.timer.every(2000,cx => { if (!this.closed) this.store.refresh(cx,this); });
    cx.timer.every(1000,cx => { if (!this.closed && this.model().working) cx.notify(); });
  }
  close() { this.closed = true; if (this.details && dialog === this.overlayId) dialog = null; }
  open() { if (!dialog) dialog = openOverlay({mod:MOD_NAME,props:{view:'log'},kind:'dialog',width:680,height:760}); }
  dismiss() { if (this.overlayId) closeOverlay(this.overlayId); }
  model() { return railModel({...this.store,now:Date.now(),expandedDays:this.expandedDays}); }
  didRefresh(cx) {
    const vm = this.model(), keys = vm.rows.map(row => row.key).join('|');
    const availabilityChanged = this.lastAvailable !== vm.available;
    this.lastAvailable = vm.available;
    if (availabilityChanged) this.follow = true;
    const nextInput = vm.working ? this.busyInput : this.idleInput;
    if (nextInput !== this.input) {
      const focused = this.inputFocused;
      nextInput.set_value(this.draft); this.input = nextInput;
      if (focused) nextInput.focus();
    }
    if (!this.store.status?.coordinator?.restart_pending) this.confirmRestart = false;
    if (this.question && !vm.questions.some(row => row.questionId === this.question.id)) this.question = null;
    if ((keys !== this.lastRows || availabilityChanged) && this.follow) {
      this.windowStart = Math.max(0,vm.rows.length-100); this.pinBottom = true;
    }
    this.lastRows = keys; cx.notify();
  }
  latest(cx) {
    this.follow = true; this.windowStart = Math.max(0,this.model().rows.length-100);
    this.pinBottom = true; cx.notify();
  }
  logMetrics(vm = this.model()) {
    let bottom = 18 + (this.windowStart > 0 ? 24 : 0);
    for (const row of vm.rows.slice(this.windowStart,this.windowStart+100)) bottom += this.rowHeights.get(row.key) || 72;
    return {maxScroll:Math.max(0,bottom+64-this.logHeight)};
  }
  scrolled(delta,cx) {
    const metrics = this.logMetrics(), offset = this.follow ? metrics.maxScroll : this.scrollTop;
    this.scrollTop = Math.max(0,Math.min(metrics.maxScroll,offset-delta));
    this.follow = this.scrollTop >= metrics.maxScroll-1; cx.notify();
  }
  older(cx) { this.follow = false; this.scrollTop = 0; this.windowStart = Math.max(0,this.windowStart-100); this.scroll.scroll_to_item(0); cx.notify(); }
  jumpReceipt(key,cx) {
    const vm = this.model(); let index = vm.rows.findIndex(item => item.key === key);
    if (index < 0) {
      for (const row of vm.rows) if (row.kind === 'day' && row.collapsed) this.expandedDays.add(row.day);
      index = this.model().rows.findIndex(item => item.key === key);
    }
    if (index < 0) return;
    this.follow = false; this.windowStart = Math.max(0,index-5);
    this.scroll.scroll_to_item(index-this.windowStart+(this.windowStart > 0 ? 1 : 0)); cx.notify();
  }
  chooseReply(row,option,cx) {
    if (row.kind !== 'question' || !row.questionId || !row.options?.includes(option)) return;
    this.store.send(cx,this,{text:option,question:{id:row.questionId}});
  }
  answer(row,cx) {
    if (row.kind !== 'question' || !row.questionId) return;
    this.question = {id:row.questionId,time:clock(row.ts_ms),text:row.text};
    this.input.focus(); cx.notify();
  }
  toggle(set,key,cx) { if (set.has(key)) set.delete(key); else set.add(key); cx.notify(); }
  toggleReceipt(key,cx) {
    const opening = !this.expandedReceipts.has(key);
    this.toggle(this.expandedReceipts,key,cx);
    if (!opening) return;
    const rows = this.model().rows, index = rows.findIndex(row => row.key === key);
    // The newest receipts open below the fold: follow the bottom as the trace grows.
    if (index >= rows.length-2) this.latest(cx);
    else if (index >= 0) { this.follow = false; this.scroll.scroll_to_item(index-this.windowStart+(this.windowStart > 0 ? 1 : 0)); }
  }
  focusLink(link,_cx) { this.dismiss(); for (const action of focusAction(link)) run(action.name,action.args); }
  showCoordinator(_cx) {
    const terminal = this.store.status?.coordinator?.terminal;
    if (terminal) { this.dismiss(); run('agent.focus',{terminal}); }
  }
  coordinatorAction(action,cx) {
    // Run kit-coord's registered handlers; this mod never owns lifecycle logic.
    run(`${DEPENDENCY_MOD}.${action}`); this.menuOpen = false; this.confirmRestart = false;
    this.store.notice = 'Request sent. Coordinator status will confirm the change.';
    this.store.noticeUntil = Date.now()+6000;
    cx.notify();
  }
  toggleMotion(cx) { this.reducedMotion = !this.reducedMotion; setShared(MOTION_KEY,this.reducedMotion); cx.notify(); }
  key(event,cx) {
    if (event.modifiers?.control || event.modifiers?.alt || event.modifiers?.platform) return;
    if (event.key === 'escape') {
      if (this.confirmRestart) this.confirmRestart = false;
      else if (this.menuOpen) { this.menuOpen = false; this.menuFocus.focus(); }
      else this.dismiss();
      cx.stop_propagation(); cx.notify();
    }
  }
  render() {
    if (!this.details) return div().w(0).h(0);
    return div().id('coord-dialog-root').size_full().min_w(0).min_h(0)
      .on_size_change((size,cx) => {
        if (this.width === size.width && this.height === size.height) return;
        this.width = size.width; this.height = size.height;
        if (this.follow) this.latest(cx); else cx.notify();
      }).child(renderRail(this));
  }
}
