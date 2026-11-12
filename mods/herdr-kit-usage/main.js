import { View, div, svg } from 'gpui-kit';
import { h_flex, v_flex, Button, ScrollHandle } from 'gpui-base';
import { agents, spaces, theme, props, shared, setShared, exec, openOverlay, closeOverlay, nextCommand } from 'herdr';
import { BINARY, DATA_DIR, MOD_NAME } from './runtime.js';
import { decodeSnapshot, views, summary, bestSegment, usedFraction, value, label, credits, pace, staleLimit, span, reset, errorText, formatTime, formatDateTime } from './model.js';

const KEY = 'herdr-kit.usage.snapshot.v1';
let strips = 0, inflight = false, maintaining = false, validated = false, dialog = null;
const args = () => DATA_DIR ? ['--data-dir', DATA_DIR] : [];
const working = () => agents().some(a => a.status === 'working');
function readShared() { try { const s = shared(KEY); return s ? decodeSnapshot(JSON.stringify(s)) : null; } catch (_) { return null; } }
function statusText(p) { return ({pending:'…',not_signed_in:'Not signed in to OMP',unsupported:'Not reported for this account',unavailable:errorText(p.error)})[p.state] || 'No usage report available'; }
function segmentText(s, compact = false) { if (!s) return '—'; const parts = [label(s.limit),value(s.limit)]; const c = credits(s.limit); if (c && !compact) parts.push(c); if (s.pace?.runsOutMs && !compact) parts.push(`out ${span(s.pace.runsOutMs)}`); if (s.stale) parts.push(compact ? '~' : 'stale'); return parts.join(' '); }
function fullLabel(all, offline) { return `Subscription limits, percentages used. ${all.map(v => `${v.name}: ${v.segments.length ? v.segments.map(s => segmentText(s)).join(', ') : statusText(v.provider)}`).join('. ')}${offline ? '. Stale, last check failed' : ''}. Open details`; }
function segmentColor(s,t) { return s?.used > .95 ? t.blocked : s?.used > .8 ? t.working : t.accent; }
function meter(s,t,width) { if (!s || s.used === null || s.limit.unlimited) return div().w(width).h(3); return div().w(width).h(3).rounded(2).bg(t.border).child(div().w(Math.max(0,width*s.used)).h(3).rounded(2).bg(segmentColor(s,t))); }

export default class Usage extends View {
  init(_p,cx) {
    const p = props() ?? {};
    this.details = p.view === 'details' || (!p.view && !!p.overlay);
    this.handle = cx.focus_handle(); this.width = 1000; this.lastRead = 0; this.collectingSince = 0; this.lastWorking = working(); this.failed = false; this.closed = false;
    if (this.details) { this.scroll = ScrollHandle.new(); this.scrollFocus = cx.focus_handle(); this.scrollIndex = 0; }
    if (this.details) { if (p?.overlay) dialog = p.overlay; this.handle.focus(); }
    else strips++;
    if (!this.details || strips === 0) this.read(cx);
    if (!this.details) { this.maintain(cx); cx.timer.every(21600000, cx => this.maintain(cx)); }
    cx.timer.every(3000, cx => {
      const active = !this.details || strips === 0;
      if (!active) return;
      const now = Date.now(), w = working(), snapshot = readShared();
      if (snapshot?.collecting && !this.collectingSince) this.collectingSince = now;
      if (!snapshot?.collecting) this.collectingSince = 0;
      const followup = this.collectingSince && now - this.collectingSince < 45000;
      if ((!this.lastWorking && w) || now - this.lastRead >= (followup ? 3000 : 60000)) this.read(cx);
      this.lastWorking = w;
    });
    cx.timer.every(60000, cx => cx.notify());
    if (!this.details) cx.spawn(async cx => { while (!this.closed) { const cmd = await nextCommand(); if (this.closed) break; if (cmd === 'show') this.open(); else if (cmd === 'focus') this.handle.focus(); } });
  }
  close() { this.closed = true; if (this.details) { if (dialog === props()?.overlay) dialog = null; } else strips = Math.max(0,strips-1); }
  open() { if (!dialog) dialog = openOverlay({mod:MOD_NAME,props:{view:'details'},kind:'dialog',width:560,height:480}); }
  read(cx) {
    if (inflight) return;
    this.lastRead = Date.now(); inflight = true;
    cx.spawn(async cx => {
      try {
        const result = await exec(BINARY,[...args(),'usage','snapshot','--json',...(working() ? ['--agents-working'] : [])],{timeout:45000});
        if (result.code !== 0) throw new Error('unavailable');
        const snapshot = decodeSnapshot(result.stdout); validated = true; this.failed = false; setShared(KEY,snapshot);
      } catch (_) { this.failed = true; validated = false; }
      finally { inflight = false; cx.notify(); }
    });
  }
  maintain(cx) {
    if (maintaining) return; maintaining = true;
    cx.spawn(async () => { try { await exec(BINARY,[...args(),'update','maintain'],{timeout:600000}); } catch (_) {} finally { maintaining = false; } });
  }
  statusbar(snapshot,offline,t,now) {
    const all = views(snapshot,now,offline);
    const visible = all.filter(v => v.provider.state !== 'not_signed_in');
    const narrow = this.width < 500;
    const worst = summary(all);
    const selected = worst && (bestSegment(worst.segments,true) || bestSegment(worst.segments));
    const description = fullLabel(all,offline);
    const readout = Button.new('usage-readout').track_focus(this.handle).tab_index(0)
      .w(narrow ? 120 : 200).h(26).px(6).items_center()
      .bg(t.chrome).rounded(3).border(1)
      .border_color(this.handle.is_focused() ? t.accent : t.chrome)
      .hover(s => s.bg(t.hover).text_color(t.foreground))
      .text_size(11.8).text_color(t.dim).tooltip(description)
      .accessibility_label(description).on_click(() => this.open());
    if (!snapshot) {
      readout.child(this.failed ? 'Limits unavailable' : 'Limits …');
    } else if (!visible.length) {
      readout.child('Limits');
    } else {
      const amount = selected ? value(selected.limit)+(selected.stale ? ' ~' : '') : worst?.provider.state === 'unsupported' ? 'n/a' : '—';
      readout.child(h_flex().gap(5).items_center()
        .child('Limits')
        .when(!narrow && Boolean(worst),el => el.child(div().text_color(t.muted).child(worst.name)))
        .child(div().text_color(selected?.used > .8 ? segmentColor(selected,t) : t.dim).child(amount))
        .when(visible.length > 1,el => el.child(`+${visible.length-1}`)));
    }
    const space = spaces().find(s => s.active);
    const list = agents();
    const needs = list.filter(a => a.status === 'blocked').length;
    const statusLabel = (icon,text) => h_flex().gap(6).items_center()
      .child(svg(`mods/${MOD_NAME}/${icon}.svg`).size(13))
      .child(div().min_w(0).truncate().child(text));
    const left = h_flex().flex_1().min_w(0).gap(16).items_center().overflow_hidden()
      .child(statusLabel(space?.machine ? 'server' : 'monitor',space?.machine ?? 'This computer'))
      .when(Boolean(space?.branch),el => el.child(div().max_w(160).min_w(0).child(statusLabel('branch',space.branch))))
      .child(h_flex().gap(6).items_center()
        .child(div().size(6).rounded(3).bg(t.dim)).child(`${list.length} agents`)
        .when(needs > 0,el => el.child(div().text_color(t.blocked).child(`· ${needs} needs you`))))
      .child(div().flex_1());
    return h_flex().size_full().px(14).gap(16).items_center().bg(t.chrome)
      .border_t(1).border_color(t.border).text_size(11.8).text_color(t.muted)
      .child(left).child(readout);
  }
  detailLimit(l, account, p, offline, t, now) {
    const stale = offline || p.stale || p.error !== null || staleLimit(l,account.fetched_at_ms,now), used = usedFraction(l), estimate = pace(l,now,stale), r = reset(l.resets_at_ms,now), c = credits(l,true);
    const desc = `${label(l)}, ${l.unlimited ? 'Unlimited' : used === null ? 'Usage unknown' : value(l)+' used'}${c ? ', '+c : ''}${r ? ', '+r : ''}${stale ? ', stale' : ''}${estimate?.runsOutMs ? ', estimated to run out in '+span(estimate.runsOutMs) : ''}`;
    const row = v_flex().gap(4).accessibility_label(desc).child(h_flex().gap(8).items_center().child(div().flex_1().child(label(l))).child(div().child(l.unlimited ? 'Unlimited' : used === null ? 'Usage unknown' : `${value(l)} used`)));
    if (used !== null) row.child(meter({limit:l,used},t,this.width < 440 ? Math.max(80,Math.min(300,this.width-50)) : 180));
    if (c) row.child(div().text_color(t.muted).child(c));
    if (estimate?.runsOutMs) row.child(div().text_color(t.muted).child(`Estimated to run out in ${span(estimate.runsOutMs)}`));
    if (r) row.child(div().text_color(t.muted).child(r));
    if (stale && !staleLimit(l,account.fetched_at_ms,now)) row.child(div().text_color(t.muted).child('Stale · awaiting a newer report'));
    return row;
  }
  detail(snapshot,offline,t,now) {
    const all = views(snapshot,now,offline), report = snapshot ? ` · updated ${formatTime(snapshot.generated_at_ms)}` : '';
    const body = v_flex().id('usage-body').flex_1().min_h(0).overflow_y_scrollbar().track_scroll(this.scroll).track_focus(this.scrollFocus).tab_index(1).gap(12).text_size(13).text_color(t.foreground).on_key_down((e,cx) => {
      if (e.key === 'down' || e.key === 'pagedown') this.scrollIndex = Math.min(2,this.scrollIndex+1);
      else if (e.key === 'up' || e.key === 'pageup') this.scrollIndex = Math.max(0,this.scrollIndex-1);
      else if (e.key === 'home') this.scrollIndex = 0;
      else if (e.key === 'end') this.scrollIndex = 2;
      else return;
      this.scroll.scroll_to_item(this.scrollIndex); cx.stop_propagation();
    });
    if (!snapshot) body.child(div().text_color(t.muted).child(this.failed ? 'Usage unavailable' : 'Reading limits…'));
    else for (const v of all) {
      const p = v.provider, section = v_flex().gap(8).border_t(1).border_color(t.border).pt(10).child(h_flex().justify_between().child(div().font_semibold().child(v.name)).child(div().text_color(t.muted).child(p.fetched_at_ms === null ? '' : `Report ${formatTime(p.fetched_at_ms)}`)));
      if (p.error) section.child(div().text_color(t.muted).child(`Last check failed: ${errorText(p.error)}`));
      if (!p.accounts.length) section.child(div().text_color(t.muted).child(statusText(p)));
      p.accounts.forEach((a,i) => { if (p.accounts.length > 1) section.child(div().font_semibold().child(`Account ${i+1} · ${formatDateTime(a.fetched_at_ms)}`)); if (!a.limits.length) section.child(div().text_color(t.muted).child('No usage report available')); for (const l of a.limits) section.child(this.detailLimit(l,a,p,offline,t,now)); });
      body.child(section);
    }
    return v_flex().id('usage-details').size_full().min_h(0).p(14).gap(8).bg(t.popover).text_color(t.foreground).text_size(13).role('dialog').accessibility_label('Subscription limits').child(h_flex().items_center().child(div().flex_1().font_semibold().text_size(15).child('Subscription limits')).child(Button.new('usage-close').track_focus(this.handle).tab_index(0).px(8).py(4).rounded(4).border(1).border_color(this.handle.is_focused()?t.accent:t.border).hover(s => s.bg(t.hover)).accessibility_label('Close subscription limits').on_click(() => closeOverlay(props().overlay)).child('Close'))).child(div().text_color(t.muted).child(`Local OMP usage${report}${snapshot?.collecting ? ' · Checking…' : ''}${offline && snapshot ? ' · Stale' : ''}`)).child(div().text_color(t.muted).child('Usage changed elsewhere may appear after a later check.')).child(body);
  }
  render() {
    const snapshot = readShared(), offline = !validated || this.failed, t = theme(), now = Date.now();
    return div().id('usage-root').size_full().on_size_change((size,cx) => { if (this.width !== size.width) { this.width=size.width; cx.notify(); } }).child(this.details ? this.detail(snapshot,offline,t,now) : this.statusbar(snapshot,offline,t,now));
  }
}
