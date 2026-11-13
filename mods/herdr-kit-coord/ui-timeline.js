import { div } from 'gpui-kit';
import { h_flex, v_flex, Input } from 'gpui-base';
import { theme } from 'herdr';
import { timelineViewModel, timelineWindow } from './model.js';
import { actionButton, banner, detailsPanel, dialog, list, loadingState } from './ui-board.js';

function scope(view, value, cx) {
  view.ui.scope = value;
  view.store.setScope(value);
  view.store.refresh(cx, 'log', true);
  cx.notify();
}
function timelineRow(row, view, t) {
  if (row.kind === 'day') return div().pt(8).text_size(11).font_semibold().text_color(t.muted).child(row.title);
  if (row.kind === 'more') return row.selectable ? actionButton('timeline-more', row.title || 'Load older entries (200)', t, cx => view.store.refresh(cx, 'log', true, {mode:'append'})) : div().text_color(t.dim).child(row.title);
  const selected = view.ui.selectedKey === row.key, expanded = view.ui.expanded.has(row.key);
  const color = ({attention:t.blocked,warning:t.working,ok:t.done,neutral:t.foreground,quiet:t.dim})[row.tone] || t.foreground;
  const line = h_flex().gap(6).items_center().min_w(0).h(24)
    .child(div().w(64).flex_shrink_0().text_color(t.dim).text_size(11).child(row.time || ''))
    .child(div().w(14).flex_shrink_0().text_color(color).child(row.glyph || '·'));
  if (view.width >= 640) line.child(div().w(84).flex_shrink_0().truncate().text_color(t.muted).text_size(11).child(row.actorLabel || ''));
  if (row.scopeId) line.child(actionButton(`scope-${row.key}`,row.scopeId,t,cx => scope(view,row.source?.task ? {task:row.source.task} : {ws:row.source?.ws},cx)));
  line.child(div().flex_1().min_w(0).truncate().text_color(color).tooltip(row.text || row.title || '').child(row.text || row.title || ''));
  const wrap = v_flex().id(`timeline-row-${row.key}`).role('list_item').min_w(0).px(4).rounded(4).border(1).border_color(selected ? t.accent : t.popover)
    .bg(selected ? t.selected : t.popover).hover(s => s.bg(t.hover)).aria_selected(selected)
    .accessibility_label([row.time, row.actorLabel, row.scopeId, row.text].filter(Boolean).join(', '))
    .on_click((_e,cx) => view.select(row,cx)).child(line);
  if (selected) wrap.aria_active_descendant();
  if (expanded) wrap.child(detailsPanel(row,view,t));
  return wrap;
}
export function renderTimeline(view) {
  const t = theme(), store = view.store;
  const vm = timelineViewModel(store.log,{...view.ui,failed:store.logFailed,error:store.logError,status:store.status,statusFailed:store.failed,statusError:store.error,starting:store.starting,newerCount:store.newCount});
  const body = list(view,'coord-timeline-list'), content = v_flex().flex_1().min_h(0).gap(4);
  view.scrollIndices = new Map();
  const controls = h_flex().gap(6).items_center();
  const id = view.ui.scope?.task || view.ui.scope?.ws;
  if (id) controls.child(actionButton('clear-scope',`Scope: ${id} ✕`,t,cx => scope(view,{},cx)));
  controls.child(div().flex_1().min_w(0).child(Input.new(view.filter).tab_index(5)));
  controls.child(actionButton('timeline-detail',view.ui.detail === 'key' ? `Key events · ${vm.hiddenCount || 0} hidden` : 'Everything',t,cx => { view.ui.detail = view.ui.detail === 'key' ? 'all' : 'key'; cx.notify(); }));
  content.child(controls);
  if (vm.newerPill) content.child(h_flex().justify_center().child(actionButton('timeline-new',vm.newerPill,t,cx => view.acceptNew(cx),true)));
  if (vm.banner) body.child(banner(vm.banner,t,view,vm.bannerAction));
  if (vm.empty) {
    if (vm.empty.kind === 'loading') body.child(loadingState('Reading journal',t));
    else {
      const state = v_flex().flex_1().gap(8).items_center().justify_center().text_color(t.muted);
      if (vm.empty.kind === 'error') state.role('alert');
      state.child(div().child(vm.empty.text));
      if (vm.empty.detail) state.child(div().child(vm.empty.detail));
      if (vm.empty.action) state.child(actionButton('timeline-empty-action',vm.empty.action,t,cx => {
        if (vm.empty.action === 'Clear filter') { view.filter.set_value(''); view.ui.text=''; cx.notify(); }
        else if (vm.empty.action === 'Clear scope') scope(view,{},cx);
        else view.refresh(cx);
      }));
      body.child(state);
    }
  }
  {
    const window = timelineWindow(vm.rows,view.ui.selectedKey,view.timelineStart || 0);
    view.timelineStart = window.start;
    let index = (vm.banner ? 1 : 0) + (vm.empty ? 1 : 0);
    const shift = (start, cx) => {
      const row = vm.rows.slice(start, start + 120).find(r => r.selectable);
      view.timelineStart = start;
      if (row) view.ui.selectedKey = row.key;
      view.scroll.scroll_to_item(0);
      cx.notify();
    };
    if (window.newer) { body.child(actionButton('timeline-window-newer','Show newer loaded entries',t,cx => shift(Math.max(0,window.start-120),cx))); index++; }
    for (const row of vm.rows.slice(window.start,window.end)) { view.scrollIndices.set(row.key,index++); body.child(timelineRow(row,view,t)); }
    if (window.older) body.child(actionButton('timeline-window-older','Show older loaded entries',t,cx => shift(window.end,cx)));
  }
  content.child(body);
  return dialog(view,content,vm.empty?.kind === 'loading' ? 'Esc close' : '↑↓ select · Enter details · s scope · c copy · a all events · b board · r refresh · Esc close');
}
