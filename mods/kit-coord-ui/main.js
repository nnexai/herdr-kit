import { View, div, svg } from 'gpui-kit';
import { Button, ScrollHandle, TextareaState, InputState } from 'gpui-base';
import { props, theme, shared, setShared, run, openOverlay, closeOverlay, nextCommand } from 'herdr';
import { MOD_NAME, DEPENDENCY_MOD } from './runtime.js';
import { railModel, focusAction, clock } from './model.js';
import { RailStore } from './store.js';
import { renderRail } from './render.js';
import { systemReducedMotion } from './motion.js';

const MOTION_KEY = 'herdr-kit.coord-ui.reduced-motion';
let dialog = null;
export default class MissionLog extends View {
  init(_props,cx) {
    const p = props() || {};
    this.details = p.view === 'log' || (!p.view && !!p.overlay);
    this.closed = false; this.width = this.details ? 680 : 240;
    this.overlayId = p.overlay; this.rootFocus = cx.focus_handle(); this.menuFocus = cx.focus_handle();
    this.menuOpen = false; this.confirmRestart = false;
    if (!this.details) {
      this.needsCount = shared('herdr-kit.coord.status.v1')?.counts?.needs_you || 0;
      cx.timer.every(2000,cx => {
        const count = shared('herdr-kit.coord.status.v1')?.counts?.needs_you || 0;
        if (!this.closed && count !== this.needsCount) { this.needsCount=count; cx.notify(); }
      });
      cx.spawn(async cx => { while (!this.closed) { const command = await nextCommand(); if (this.closed) break; if (command === 'show') this.open(); else if (command === 'focus') this.rootFocus.focus(); } });
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
      this.windowStart = Math.max(0,vm.rows.length-100);
      this.scroll.scroll_to_item(vm.rows.length-this.windowStart+(this.windowStart > 0 ? 1 : 0)+(vm.rows.length ? 1 : 2));
    }
    this.lastRows = keys; cx.notify();
  }
  latest(cx) {
    const count = this.model().rows.length;
    this.follow = true; this.windowStart = Math.max(0,count-100);
    this.scroll.scroll_to_item(count-this.windowStart+(this.windowStart > 0 ? 1 : 0)+(count ? 1 : 2)); cx.notify();
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
  focusLink(link,_cx) { this.dismiss(); for (const action of focusAction(link)) run(action.name,action.args); }
  showCoordinator(_cx) {
    const terminal = this.store.status?.coordinator?.terminal;
    if (terminal) { this.dismiss(); run('agent.focus',{terminal}); }
  }
  coordinatorAction(action,cx) {
    // Reuse the Board's registered handlers; this mod never owns lifecycle logic.
    run(`${DEPENDENCY_MOD}.${action}`); this.menuOpen = false; this.confirmRestart = false;
    this.store.notice = 'Request sent. Coordinator status will confirm the change.';
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
    const t = theme();
    const root = div().id('mission-log-root').size_full().min_w(0).min_h(0)
      .on_size_change((size,cx) => {
        if (this.width === size.width && this.height === size.height) return;
        this.width = size.width; this.height = size.height;
        if (this.details && this.follow) this.latest(cx); else cx.notify();
      });
    if (this.details) return root.child(renderRail(this));
    const launcher = Button.new('mission-log-open').size_full().px(12).gap(8).justify_start().items_center()
      .track_focus(this.rootFocus).tab_index(0).text_color(t.muted).text_size(12)
      .hover(s => s.bg(t.hover).text_color(t.foreground)).tooltip('Coordinator')
      .accessibility_label('Open Coordinator').on_click(() => this.open())
      .child(svg(`mods/${MOD_NAME}/rail.svg`).w(16).h(22).text_color(t.accent));
    if (this.width > 80) launcher.child(div().flex_1().min_w(0).truncate().child('Coordinator'));
    const count = shared('herdr-kit.coord.status.v1')?.counts?.needs_you || 0;
    if (count) launcher.child(div().px(5).rounded(8).bg(t.blocked).text_color(t.accent_foreground).text_size(11).child(String(count)));
    return root.bg(t.chrome).child(launcher);
  }
}
