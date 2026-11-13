import { div } from 'gpui-kit';
import { h_flex, v_flex, Button } from 'gpui-base';
import { theme, props, closeOverlay } from 'herdr';
import { boardViewModel, formatAge, formatClock, clip, oneLine } from './model.js';

export function actionButton(id, label, t, handler, primary = false, tabIndex = 6) {
  return Button.new(id).tab_index(tabIndex).h(22).px(8).rounded(4).border(1).border_color(primary ? t.accent : t.border)
    .bg(primary ? t.accent : t.popover).text_color(primary ? t.accent_foreground : t.foreground)
    .text_size(11).hover(s => s.bg(t.hover).text_color(t.foreground)).accessibility_label(label)
    .on_click((_e, cx) => { cx.stop_propagation(); handler(cx); }).child(label);
}
export function dialog(view, body, footer) {
  const t = theme(), status = view.store.status, starting = view.store.starting;
  const state = starting ? 'Starting…' : !status ? 'Loading…' : !status.enabled ? 'Not running' : status.state === 'running' ? 'Running' : status.state === 'degraded' ? 'Degraded' : 'Not running';
  const color = starting ? t.dim : status?.enabled && status.state === 'running' ? t.accent : status?.enabled && status.state === 'degraded' ? t.working : t.dim;
  const header = h_flex().gap(8).items_center().child(div().font_semibold().text_size(15).child('Coordinator'));
  const tabs = h_flex().gap(2).role('tab_list');
  for (const [id, label] of [['board','Board'],['timeline','Timeline']]) {
    const selected = view.ui.view === id;
    tabs.child(Button.new(`coord-tab-${id}`).role('tab').aria_selected(selected).selected(selected).tab_index(id === 'board' ? 1 : 2)
      .px(8).py(4).rounded(4).bg(selected ? t.selected : t.popover).hover(s => s.bg(t.hover))
      .on_click((_e, cx) => view.setTab(id,cx)).child(label));
  }
  header.child(tabs).child(div().flex_1()).child(h_flex().gap(5).items_center().text_color(color).child(div().size(6).rounded(3).bg(color)).child(state));
  if (status && (!status.enabled || status.state !== 'running') && !starting) header.child(actionButton('coord-start', status.enabled && status.state === 'degraded' ? 'Repair' : 'Start coordinator', t, cx => view.start(cx), true, 3));
  if (status?.coordinator?.terminal) header.child(actionButton('coord-show', 'Show coordinator', t, cx => view.navigate('reply', {kind:'attention', source:{kind:'question'}}, cx), false, 4));
  header.child(actionButton('coord-close', 'Close', t, () => closeOverlay(props().overlay), false, 7));
  const health = [];
  if (status) {
    health.push(`Watcher ${status.watcher?.running ? (status.state === 'degraded' ? 'stalled' : 'alive') : 'not running'}`);
    if (status.watcher?.heartbeat_ms != null) health.push(`heartbeat ${formatAge(Date.now() - status.watcher.heartbeat_ms)} ago`);
    if (status.watcher?.release) health.push(`release ${status.watcher.release.slice(0,7)}`);
    if (status.coordinator?.status) health.push(`coordinator ${status.coordinator.status}`);
    if (status.generated_at_ms != null) health.push(`updated ${formatClock(status.generated_at_ms)}`);
  }
  return v_flex().id('coord-dialog').size_full().min_h(0).p(14).gap(8).bg(t.popover).text_color(t.foreground).text_size(13)
    .role('dialog').accessibility_label('Coordinator').child(header)
    .child(div().role('status').min_w(0).truncate().text_size(11.8).text_color(t.muted).tooltip(health.join(' · ')).child(health.join(' · ')))
    .child(body).child(div().text_size(11).text_color(t.dim).child(footer));
}
export function list(view, id) {
  return v_flex().id(id).role('list').flex_1().min_h(0).gap(4).overflow_y_scrollbar().track_scroll(view.scroll)
    .track_focus(view.focus).tab_index(6).on_key_down((e,cx) => view.key(e,cx));
}
export function loadingState(label, t) {
  const frames = ['⠋','⠙','⠹','⠸','⠼','⠴','⠦','⠧','⠇','⠏'].map(frame => `${label} ${frame}`);
  return h_flex().flex_1().items_center().justify_center().text_color(t.muted).child(div().frames(frames, 90));
}
export function detailsPanel(row, view, t) {
  const panel = v_flex().gap(3).p(8).bg(t.surface).text_size(11);
  const details = row.details || [];
  for (const [i, detail] of details.slice(0,24).entries()) {
    const line = h_flex().gap(8).child(div().w(90).flex_shrink_0().text_color(t.dim).child(String(detail.label || '')))
      .child(detail.copy ? div().flex_1().min_w(0).text_ellipsis_middle().tooltip(String(detail.value ?? '')).child(clip(detail.value,600)) : div().flex_1().min_w(0).child(clip(detail.value,600)));
    if (detail.copy) line.child(actionButton(`copy-${row.key}-${i}`, 'Copy', t, cx => cx.write_to_clipboard(typeof detail.copy === 'string' ? detail.copy : String(detail.value ?? ''))));
    panel.child(line);
  }
  if (details.length > 24) panel.child(div().text_color(t.dim).child(`+${details.length - 24} more`));
  return panel;
}
export function banner(text, t, view, action = 'Retry') {
  return h_flex().gap(8).p(8).items_center().rounded(4).border(1).border_color(t.border).bg(t.raised).role('alert')
    .child(div().flex_1().min_w(0).child(text))
    .child(actionButton(`coord-banner-${action}`, action, t, cx => action === 'Repair' || action === 'Start coordinator' ? view.start(cx) : view.refresh(cx)));
}
function boardRow(row, view, t) {
  const selected = view.ui.selectedKey === row.key, expanded = view.ui.expanded.has(row.key);
  const wrap = v_flex().id(`coord-row-${row.key}`).role(row.kind === 'workstream' ? 'tree_item' : 'list_item').min_w(0).p(5).rounded(5).border(1).border_color(selected ? t.accent : t.popover)
    .bg(selected ? t.selected : t.popover).hover(s => s.bg(t.hover)).aria_selected(selected)
    .accessibility_label([row.kind, row.source?.id, row.state?.word, row.age, row.title, row.role, row.meta].filter(Boolean).join(', '))
    .on_click((_e,cx) => { view.select(row,cx); if (row.kind === 'workstream' || row.kind === 'closed') view.navigate('toggle',row,cx); });
  if (selected) wrap.aria_active_descendant();
  const line = h_flex().gap(8).items_center().min_w(0);
  if (row.depth) line.child(div().w(row.depth * 20).flex_shrink_0());
  if (row.kind === 'workstream') line.child(div().text_color(t.dim).child(row.collapsed ? '▸' : '▾'));
  else if (row.kind === 'closed') line.child(div().text_color(t.dim).child(view.ui.showClosed.has(row.source.id) ? '▾' : '▸'));
  if (row.state && (row.kind === 'task' || row.kind === 'attention')) line.child(div().when(row.kind === 'task', d => d.w(74)).when(row.kind === 'attention' && row.state.role !== 'dim', d => d.w(14)).max_w(120).truncate().flex_shrink_0().text_color(t[row.state.role] || t.unknown).child(row.kind === 'attention' ? row.state.word : `${row.state.word === 'dormant' ? '○' : '●'} ${row.state.word}`));
  const title = div().flex_1().min_w(0).truncate().font_semibold().tooltip(clip(row.tooltip || row.title || '',600)).child(clip(oneLine(row.title),240));
  if (row.kind === 'task' && row.state?.word === 'closed') title.line_through().text_color(t.dim);
  line.child(title);
  if (row.kind === 'task' && row.role && view.width >= 640) line.child(div().max_w(96).truncate().text_size(11).text_color(t.muted).tooltip([row.role,row.model,row.harness].filter(Boolean).join(' · ')).child(row.role));
  if (row.age) line.child(div().text_color(t.muted).text_size(11).child(`${row.kind === 'workstream' || row.kind === 'retained' ? `${row.state?.word} · ` : ''}${row.age}`));
  wrap.child(line);
  if (row.kind === 'task' && row.role && view.width < 640) wrap.child(div().text_size(11).text_color(t.muted).tooltip([row.role,row.model,row.harness].filter(Boolean).join(' · ')).child(row.role));
  if (row.meta) wrap.child(div().min_w(0).truncate().text_size(11).text_color(row.kind === 'retained' ? t.working : t.dim).tooltip(clip(row.tooltip || row.meta,600)).child(clip(oneLine(row.meta),240)));
  if (row.kind === 'retained' && row.checked) wrap.child(div().text_size(11).text_color(t.dim).child(`Last checked ${row.checked} ago`));
  if (row.actions?.length) {
    const actions = h_flex().gap(4).items_center();
    for (const action of row.actions) if (action.enabled !== false) actions.child(actionButton(`${row.key}-${action.id}`, action.label, t, cx => view.navigate(action.id,row,cx), action.primary));
    wrap.child(actions);
  }
  if (expanded && row.details?.length) wrap.child(detailsPanel(row, view, t));
  return wrap;
}
export function renderBoard(view) {
  const t = theme(), store = view.store;
  const vm = boardViewModel(store.board,store.status,{...view.ui,failed:store.boardFailed,error:store.boardError,statusFailed:store.failed,statusError:store.error,starting:store.starting});
  const body = list(view,'coord-board-list');
  view.scrollIndices = new Map();
  let scrollIndex = vm.banner ? 1 : 0;
  if (vm.banner) body.child(banner(vm.banner,t,view,vm.bannerAction));
  if (vm.empty) {
    if (vm.empty.kind === 'loading') body.child(loadingState('Reading board',t));
    else {
      const state = v_flex().flex_1().gap(8).items_center().justify_center().text_color(t.muted);
      if (vm.empty.kind === 'error') state.role('alert');
      state.child(div().child(vm.empty.text));
      if (vm.empty.detail) state.child(div().child(vm.empty.detail));
      if (vm.empty.action) state.child(actionButton('board-empty-action',vm.empty.action,t,cx => {
        if (vm.empty.action === 'Start coordinator') view.start(cx);
        else if (vm.empty.action === 'Show coordinator') view.navigate('reply',{kind:'attention',source:{kind:'question'}},cx);
        else view.refresh(cx);
      },vm.empty.action === 'Start coordinator'));
      body.child(state);
    }
  } else {
    for (const section of vm.sections) {
      body.child(div().pt(8).text_size(11).font_semibold().text_color(t.muted).child(`${section.title.toUpperCase()} · ${section.count}`));
      scrollIndex++;
      for (const row of section.rows) { view.scrollIndices.set(row.key,scrollIndex++); body.child(boardRow(row,view,t)); }
    }
  }
  return dialog(view,body,vm.empty?.kind === 'loading' ? 'Esc close' : '↑↓ select · Enter show · → details · p peek · l log · t timeline · r refresh · Esc close');
}
