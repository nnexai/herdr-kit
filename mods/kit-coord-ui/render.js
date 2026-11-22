import { div, svg } from 'gpui-kit';
import { h_flex, v_flex, Button, Checkbox, Textarea, TextView, Popover, Input } from 'gpui-base';
import { theme, settings } from 'herdr';
import { MOD_NAME } from './runtime.js';
import { clock, dayLabel, journalRows, journalFields, filterRoles, roleSummaryLine, ROLE_HARNESSES, ROLE_SESSIONS } from './model.js';

const tint = (color, alpha) => (color.length === 4 ? '#' + [...color.slice(1)].map(c => c+c).join('') : color.slice(0,7)) + alpha;
const mono = element => element.font_family('JetBrains Mono');
const palette = () => {
  const t = theme();
  return settings().dark === false ? {...t,gutter:t.muted} : {...t,
    working:'#e2b94e',blocked:'#ef6a67',done:'#6fcf97',input:'#2a2d31',gutter:'#858990'};
};
export function button(id, label, handler, t, options = {}) {
  return Button.new(id).tab_index(options.tabIndex ?? 3).h(options.height || 24).px(options.px ?? 10)
    .rounded(options.pill ? 13 : 4).border(1).border_color(options.color || t.border)
    .bg(options.primary ? t.accent : options.bg || t.popover)
    .text_color(options.primary ? t.accent_foreground : options.color || t.foreground)
    .text_size(options.size || 12).disabled(!!options.disabled).flex_shrink_0()
    .hover(s => s.bg(options.primary ? t.accent : t.hover).text_color(options.primary ? t.accent_foreground : options.color || t.foreground)).accessibility_label(options.label || label)
    .on_click((_e,cx) => { cx.stop_propagation(); handler(cx); }).child(label);
}
const age = ms => {
  const s = Math.max(0,Math.floor((Date.now()-ms)/1000));
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s/60)}m` : `${Math.floor(s/3600)}h`;
};
function glyph(state,t,view) {
  const color = state === 'done' ? t.done : state === 'question' ? t.blocked : state === 'blocked' || state === 'running' ? t.working : t.muted;
  if (state === 'running') {
    const ring = svg(`mods/${MOD_NAME}/orbit.svg`).size(14).text_color(color);
    if (!view.reducedMotion) ring.spin(1200);
    return ring;
  }
  return div().w(14).text_color(color).child(({done:'✓',question:'?',blocked:'!',placed:'○'})[state] || '○');
}
function menu(view,vm,t) {
  const panel = v_flex().id('rail-menu').role('menu').w(220).p(4).gap(1).rounded(6).border(1).border_color(t.border).bg(t.raised).shadow_lg();
  const item = (id,label,action,color) => panel.child(Button.new(id).role('menu_item').w_full().h(28).px(10).rounded(4).justify_start()
    .text_size(12.5).text_color(color || t.foreground).hover(s => s.bg(t.hover)).accessibility_label(label)
    .on_click((_e,cx) => { cx.stop_propagation(); action(cx); }).child(label));
  const separator = () => panel.child(div().h(1).mx(6).my(3).bg(t.border));
  item('rail-journal','Journal',cx => { view.page='journal'; view.menuOpen=false; view.journalStart=0; view.store.journal=view.store.log; cx.notify(); });
  item('rail-roles','Roles',cx => view.openRoles(cx));
  if (view.store.status?.coordinator?.terminal) item('rail-show','Show raw coordinator',cx => view.showCoordinator(cx));
  if (vm.status !== 'off') {
    separator();
    // Clear context is the graceful restart: a fresh session once the coordinator is idle.
    if (!view.store.status?.coordinator?.restart_pending) item('rail-clear','Clear context',cx => view.coordinatorAction('clear-context',cx));
    else if (view.confirmRestart) {
      panel.child(div().px(10).py(6).text_size(12).text_color(t.muted).child('Restart now interrupts the coordinator mid-turn.'));
      item('rail-force-confirm','Confirm restart now',cx => view.coordinatorAction('restart-now',cx),t.blocked);
      item('rail-force-cancel','Cancel',cx => { view.confirmRestart=false; cx.notify(); });
    } else {
      panel.child(div().px(10).py(6).text_size(12).text_color(t.muted).child('Fresh session requested; it starts when the coordinator is idle.'));
      item('rail-force','Restart now…',cx => { view.confirmRestart=true; cx.notify(); });
    }
  }
  separator();
  item('rail-motion',`Reduce motion: ${view.reducedMotion ? 'On' : 'Off'}`,cx => view.toggleMotion(cx));
  return panel;
}
function needs(view,vm,t) {
  const items = vm.needsYou || [], wrap = v_flex().gap(6).flex_shrink_0().max_h(240).overflow_y_scrollbar();
  for (const [index,row] of (view.needsAll ? items : items.slice(0,2)).entries()) {
    const key = row.questionId || `${row.title}:${row.ts_ms}`, expanded = view.expandedQuestions.has(key);
    const text = row.text || '';
    const card = v_flex().id(`needs-${index}`).gap(4).px(10).py(8).border_l(2).border_color(t.blocked).bg(tint(t.blocked,'14')).rounded(5);
    card.child(h_flex().gap(8).items_center().text_size(12)
      .child(div().text_color(t.blocked).child('Needs you'))
      .child(div().flex_1().min_w(0).truncate().text_color(t.muted).child(row.title || ''))
      .child(div().text_color(t.muted).child(age(row.ts_ms))));
    const content = div().min_w(0).flex_shrink_0().text_size(13).line_height('18px').child(text)
      .on_size_change((size,cx) => {
        const long = size.height > 54, wasLong = view.clampedQuestions.has(key);
        if (long !== wasLong) { if (long) view.clampedQuestions.add(key); else view.clampedQuestions.delete(key); cx.notify(); }
      });
    const textBox = div().min_w(0);
    if (!expanded) textBox.max_h(54).overflow_hidden();
    card.child(textBox.child(content));
    if (view.clampedQuestions.has(key)) card.child(button(`needs-more-text-${index}`,expanded ? 'less' : 'more',cx => view.toggle(view.expandedQuestions,key,cx),t,{size:11,px:4}).border(0).self_start());
    if (row.kind === 'question') {
      const actions = h_flex().gap(6).flex_wrap();
      (row.options || []).forEach((option,i) => actions.child(button(`needs-option-${index}-${i}`,option,cx => view.chooseReply(row,option,cx),t,{pill:true,height:26,disabled:!vm.canSend || vm.working || view.store.sending})));
      actions.child(button(`needs-reply-${index}`,'Reply…',cx => view.answer(row,cx),t,{pill:true,height:26}));
      card.child(actions);
    }
    wrap.child(card);
  }
  if (items.length > 2) wrap.child(button('needs-all',view.needsAll ? 'Show less' : `${items.length-2} more`,cx => { view.needsAll=!view.needsAll; cx.notify(); },t,{size:11}).border(0));
  return wrap;
}
function receipt(view,row,t) {
  const open = view.expandedReceipts.has(row.key), active = open || view.hoveredReceipt === row.key;
  const terminal = (row.links || []).find(link => link.kind === 'terminal');
  const box = v_flex().id(row.key).ml(12).pl(12).border_l(1).border_color(t.border).gap(3).py(4)
    .on_hover((hovered,cx) => {
      const next = hovered ? row.key : view.hoveredReceipt === row.key ? null : view.hoveredReceipt;
      if (next !== view.hoveredReceipt) { view.hoveredReceipt = next; cx.notify(); }
    });
  const top = h_flex().gap(8).h(24).items_center().min_w(0);
  top.child(Button.new(`receipt-select-${row.key}`).flex_1().min_w(0).justify_start()
    .accessibility_label(`${open ? 'Hide' : 'Show'} trace for ${row.title || 'workstream'}`)
    .on_click((_e,cx) => { cx.stop_propagation(); view.toggleReceipt(row.key,cx); })
    .child(h_flex().gap(8).items_center().min_w(0).w_full().child(glyph(row.state,t,view))
      .child(div().flex_1().min_w(0).truncate().font_semibold().child(row.title || 'Workstream'))));
  // One fixed-size slot holds either the meta or the actions, so hovering never moves the row.
  const slot = h_flex().w(172).h(24).flex_shrink_0().gap(4).items_center().justify_end().overflow_hidden();
  if (!active) slot.child(div().min_w(0).truncate().text_size(11).text_color(t.muted).child([row.space,row.duration].filter(Boolean).join('  ')));
  else {
    if (terminal) slot.child(button(`terminal-${row.key}`,'Open terminal',cx => view.focusLink(terminal,cx),t,{size:11,px:4}).border(0));
    slot.child(button(`trace-${row.key}`,open ? 'Hide trace' : 'Trace',cx => view.toggleReceipt(row.key,cx),t,{size:11,px:4}).border(0));
  }
  top.child(slot);
  box.child(top).child(div().pl(22).min_w(0).truncate().text_size(12).text_color(row.state === 'question' ? t.blocked : row.state === 'blocked' ? t.working : t.muted).child(row.text || ''));
  if (open) {
    let day = null;
    for (const line of (row.trace || [])) {
      const label = dayLabel(line.ts_ms);
      if (label !== day) { day = label; box.child(div().pl(22).pt(4).text_size(11).font_semibold().text_color(t.muted).child(label)); }
      box.child(h_flex().gap(8).pl(22).text_size(12)
        .child(div().w(40).flex_shrink_0().text_color(t.muted).child(clock(line.ts_ms)))
        .child(div().flex_1().min_w(0).child(line.text || '')));
    }
    if (row.text && !(row.trace || []).some(line => line.text?.includes(row.text)))
      box.child(div().pl(70).text_size(12).text_color(t.muted).child(row.text));
    const links = h_flex().gap(8).pl(22).flex_wrap();
    (row.links || []).forEach((link,i) => links.child(button(`receipt-link-${row.key}-${i}`,link.kind === 'terminal' ? 'Open terminal' : 'Show space',cx => view.focusLink(link,cx),t,{size:11,px:4}).border(0)));
    box.child(links);
  }
  return box;
}
function entry(view,row,t) {
  let body;
  if (row.kind === 'day' && !row.collapsible) body = div().py(4).text_size(11).text_color(t.muted).child(row.label);
  else if (row.kind === 'day') {
    const label = `${row.collapsed ? '▸' : '▾'} ${row.label}${row.collapsed ? ` — ${row.countLabel}${row.summary ? `, ${row.summary}` : ''}` : ''}`;
    body = button(`day-${row.key}`,label,cx => { view.toggle(view.expandedDays,row.day,cx); view.follow=false; },t,{size:11,px:0,bg:t.popover}).w_full().justify_start().border(0).text_color(t.muted);
  } else if (row.kind === 'receipt') body = receipt(view,row,t);
  else if (row.kind === 'person') body = h_flex().gap(8).items_start().px(10).py(8).rounded(6).bg(t.raised)
    .child(div().flex_1().min_w(0).child(row.text || '')).child(div().text_size(11).text_color(t.muted).child(row.sending ? 'sending…' : clock(row.ts_ms)));
  else if (row.kind === 'reply') body = TextView.markdown(`body-${row.key}`,row.text || '').selectable(true).scrollable(false).text_size(13).line_height(1.5);
  else if (row.kind === 'echo') body = h_flex().gap(8).items_center().min_w(0).text_size(12).text_color(t.muted)
    .child(glyph(row.state,t,view)).child(div().flex_1().min_w(0).truncate().child(`${row.title || 'Workstream'}${row.text ? ` — ${row.text}` : ''}`))
    .child(button(`echo-${row.key}`,'↑',cx => view.jumpReceipt(row.receiptKey,cx),t,{label:'Show receipt',px:5}).border(0));
  else body = div().child(row.text || '');
  return v_flex().id(`entry-${row.key}`).min_w(0).flex_shrink_0().pb(8)
    .on_size_change((size,cx) => {
      if (view.rowHeights.get(row.key) === size.height) return;
      view.rowHeights.set(row.key,size.height);
      // Rows that grow at the bottom (an opened trace, a long reply) stay in view.
      if (view.follow) view.latest(cx); else cx.notify();
    }).child(body);
}
const toneGlyph = (tone, t) => {
  const cell = h_flex().w(12).h(18).flex_shrink_0().items_center().justify_center();
  return tone === 'quiet' ? cell.child(div().size(4).rounded(2).bg(t.gutter))
    : cell.text_size(12).text_color(tone === 'done' ? t.done : tone === 'question' ? t.blocked : t.working)
      .child(({done:'✓',question:'?',blocked:'!'})[tone]);
};
function journalEvent(view,row,t) {
  const open = view.expandedJournal.has(row.key);
  const sentence = div().flex_1().min_w(0).text_size(12.5).text_color(row.system ? t.muted : t.foreground).child(row.text);
  if (!open) sentence.truncate();
  const line = h_flex().w_full().gap(8).items_start().min_w(0)
    .child(div().w(38).flex_shrink_0().text_size(11.5).text_color(t.muted).child(clock(row.ts_ms)))
    .child(toneGlyph(row.tone,t)).child(sentence);
  const toggle = Button.new(`journal-${row.key}`).w_full().h_auto().px(6).py(3).rounded(4).justify_start()
    .hover(s => s.bg(t.hover)).accessibility_label(row.text)
    .on_click((_e,cx) => view.toggle(view.expandedJournal,row.key,cx));
  if (open) toggle.bg(t.hover);
  const item = v_flex().w_full().min_w(0).flex_shrink_0().child(toggle.child(line));
  if (!open) return item;
  const fields = v_flex().ml(58).mr(6).mt(2).mb(8).gap(2).text_size(11);
  for (const [label,value] of journalFields(row)) fields.child(h_flex().gap(8).items_start().min_w(0)
    .child(div().w(64).flex_shrink_0().text_color(t.muted).child(label))
    .child(div().flex_1().min_w(0).text_color(t.foreground).child(value)));
  return item.child(fields);
}
function journal(view,t) {
  const rows = journalRows(view.store.journal || view.store.log,view.store.board,{filter:view.filter.value(),showSystem:view.showSystem});
  const start = Math.min(view.journalStart || 0,Math.max(0,rows.length-1));
  const body = v_flex().id('journal-list').flex_1().min_h(0).overflow_y_scrollbar().px(12).pb(12);
  if (start) body.child(button('journal-newer','Show newer',cx => { view.journalStart=Math.max(0,start-120); cx.notify(); },t,{size:11}).mt(8));
  for (const row of rows.slice(start,start+120)) {
    if (row.kind === 'day') body.child(div().pt(14).pb(2).text_size(11).text_color(t.muted).child(row.label));
    else if (row.kind === 'group') body.child(div().pt(8).pb(2).px(6).text_size(12).font_semibold()
      .text_color(row.ws ? t.foreground : t.muted).truncate().child(row.title));
    else body.child(journalEvent(view,row,t));
  }
  if (!rows.length) body.child(div().p(12).text_color(t.muted).child(view.filter.value() ? 'No events match the filter.' : 'Nothing recorded yet.'));
  if (start+120 < rows.length) body.child(button('journal-older-loaded','Show older',cx => { view.journalStart=start+120; cx.notify(); },t,{size:11}).mt(8));
  else if (view.store.journal?.has_more) body.child(button('journal-older',view.store.loadingOlder ? 'Loading…' : 'Load older',cx => view.store.loadOlder(cx,view),t,{size:11,disabled:view.store.loadingOlder}).mt(8));
  if (view.store.journalError) body.child(div().pt(6).text_size(11).text_color(t.blocked).child(view.store.journalError));
  const indicator = h_flex().size(14).items_center().justify_center().rounded(3).border(1).border_color(view.showSystem ? t.accent : t.muted)
    .bg(view.showSystem ? t.accent : t.input).flex_shrink_0();
  if (view.showSystem) indicator.child(div().size(6).rounded(1).bg(t.accent_foreground));
  return v_flex().flex_1().min_h(0).child(h_flex().gap(10).items_center().px(12).pt(10).pb(6).border_b(1).border_color(t.border)
    .child(h_flex().flex_1().h(28).px(8).items_center().rounded(4).border(1).border_color(t.border).bg(t.input).child(Input.new(view.filter).w_full()))
    .child(Checkbox.new('journal-system').checked(!!view.showSystem).accessibility_label('Show system events')
      .on_change((checked,cx) => { view.showSystem=checked; view.journalStart=0; cx.notify(); })
      .child(h_flex().gap(6).items_center().text_size(11.5).text_color(t.muted).child(indicator).child('System events'))))
    .child(body);
}
const roleSourceColor = (source, t) => source === 'override' ? t.working : source === 'file' ? t.accent : t.muted;
function roleNoticeBar(view, t) {
  const notice = view.store.rolesNoticeUntil && Date.now() > view.store.rolesNoticeUntil ? '' : view.store.rolesNotice;
  const error = view.store.rolesError;
  if (!error && !notice) return null;
  return div().px(12).pt(8).text_size(11.5).text_color(error ? t.blocked : t.done).child(error || notice);
}
function roleRow(view, role, t) {
  const confirming = view.confirmRoleAction?.name === role.name ? view.confirmRoleAction.action : null;
  const active = !!confirming || view.hoveredRole === role.name;
  const removable = role.source === 'file' ? 'delete' : role.source === 'override' ? 'reset' : null;
  const row = v_flex().id(`role-row-${role.name}`).px(8).py(6).rounded(5).gap(2).flex_shrink_0()
    .bg(active ? t.hover : t.popover)
    .on_hover((hovered, cx) => {
      const next = hovered ? role.name : view.hoveredRole === role.name ? null : view.hoveredRole;
      if (next !== view.hoveredRole) { view.hoveredRole = next; cx.notify(); }
    });
  const top = h_flex().h(24).gap(8).items_center().min_w(0);
  top.child(Button.new(`role-open-${role.name}`).flex_1().min_w(0).h(24).justify_start()
    .accessibility_label(`View role ${role.name}`)
    .on_click((_e, cx) => { cx.stop_propagation(); view.openRoleDetail(role.name, cx); })
    .child(div().min_w(0).truncate().font_semibold().child(role.name)));
  // One fixed-size slot holds either the source label or hover/confirm actions, so hovering never shifts layout.
  const slot = h_flex().w(188).h(24).flex_shrink_0().gap(4).items_center().justify_end().overflow_hidden();
  if (confirming) {
    slot.child(button(`role-confirm-${role.name}`, confirming === 'reset' ? 'Confirm reset' : 'Confirm delete',
      cx => view.runRemoveRole(role.name, confirming, cx), t,
      {size: 11, px: 6, height: 22, color: confirming === 'reset' ? t.working : t.blocked, disabled: view.store.savingRole}).border(0));
    slot.child(button(`role-cancel-${role.name}`, 'Cancel',
      cx => { view.confirmRoleAction = null; cx.notify(); }, t, {size: 11, px: 5, height: 22}).border(0));
  } else if (active) {
    slot.child(button(`role-edit-${role.name}`, 'Edit',
      cx => view.openRoleEdit(role.name, cx), t, {size: 11, px: 6, height: 22}).border(0));
    slot.child(button(`role-copy-${role.name}`, 'Copy',
      cx => view.openRoleCreate(role.name, cx), t, {size: 11, px: 6, height: 22}).border(0));
    if (removable) slot.child(button(`role-${removable}-${role.name}`, removable === 'reset' ? 'Reset' : 'Delete',
      cx => view.confirmRemoveRole(role.name, removable, cx), t,
      {size: 11, px: 6, height: 22, color: removable === 'reset' ? t.working : t.blocked}).border(0));
  } else {
    slot.child(div().text_size(11).text_color(roleSourceColor(role.source, t)).child(role.source));
  }
  top.child(slot);
  row.child(top);
  row.child(h_flex().h(18).items_center().min_w(0).text_size(11.5).text_color(t.muted)
    .child(div().min_w(0).truncate().child(roleSummaryLine(role))));
  return row;
}
function roleList(view, t) {
  const allRoles = view.store.roles || [];
  const filtered = filterRoles(allRoles, view.roleFilter?.value?.() || '');
  const toolbar = h_flex().gap(10).items_center().px(12).pt(10).pb(6).border_b(1).border_color(t.border)
    .child(h_flex().flex_1().h(28).px(8).items_center().rounded(4).border(1).border_color(t.border).bg(t.input)
      .child(Input.new(view.roleFilter).w_full()))
    .child(button('roles-new', '+ New role', cx => view.openRoleCreate('', cx), t, {primary: true, height: 28}));
  const list = v_flex().id('roles-list').flex_1().min_h(0).overflow_y_scrollbar().px(12).pt(8).pb(12).gap(2);
  if (view.store.loadingRoles && !view.store.roles) list.child(div().p(8).text_color(t.muted).child('Loading roles…'));
  else if (!filtered.length) list.child(div().p(8).text_color(t.muted).child(
    view.roleFilter?.value?.() ? 'No roles match the filter.' : 'No roles found.'));
  else for (const role of filtered) list.child(roleRow(view, role, t));
  const root = v_flex().flex_1().min_h(0).child(toolbar);
  const banner = roleNoticeBar(view, t);
  if (banner) root.child(banner);
  return root.child(list);
}
function roleDetail(view, t) {
  const role = view.roleByName(view.selectedRole);
  if (!role) return roleList(view, t);
  const removable = role.source === 'file' ? 'delete' : role.source === 'override' ? 'reset' : null;
  const confirming = view.confirmRoleAction?.name === role.name ? view.confirmRoleAction.action : null;
  const topbar = h_flex().gap(8).items_center().px(12).pt(10).pb(6).border_b(1).border_color(t.border)
    .child(div().flex_1().min_w(0).child(h_flex().gap(8).items_center().min_w(0)
      .child(div().font_semibold().truncate().child(role.name))
      .child(div().text_size(11).text_color(roleSourceColor(role.source, t)).child(role.source))));
  if (confirming) {
    topbar.child(button('role-detail-confirm', confirming === 'reset' ? 'Confirm reset' : 'Confirm delete',
      cx => view.runRemoveRole(role.name, confirming, cx), t,
      {height: 26, color: confirming === 'reset' ? t.working : t.blocked, disabled: view.store.savingRole}));
    topbar.child(button('role-detail-cancel-remove', 'Cancel',
      cx => { view.confirmRoleAction = null; cx.notify(); }, t, {height: 26}));
  } else {
    topbar.child(button('role-detail-edit', 'Edit', cx => view.openRoleEdit(role.name, cx), t, {primary: true, height: 26}));
    topbar.child(button('role-detail-copy', 'Copy', cx => view.openRoleCreate(role.name, cx), t, {height: 26}));
    if (removable) topbar.child(button(`role-detail-${removable}`, removable === 'reset' ? 'Reset' : 'Delete',
      cx => view.confirmRemoveRole(role.name, removable, cx), t,
      {height: 26, color: removable === 'reset' ? t.working : t.blocked}));
  }
  const fields = [
    ...(role.description ? [['About', role.description, false]] : []),
    ...(role.available === false ? [['Status', role.unavailable_reason || 'Cannot start on this machine', false]] : []),
    ['Harness', role.harness || '—', false],
    ['Model', role.model || '—', true],
    ['Effort', role.effort || '—', false],
    ['Session', `${role.session || 'none'}${role.resumable ? ' · resumable' : ''}${role.steering ? ' · steering' : ''}`, false],
    ['Start', role.start || '—', true],
    ['Resume', role.resume || '—', true],
    ...(role.trust ? [['Trust', role.trust, true]] : []),
    ...(role.fallback?.length ? [['Fallback', role.fallback.join(' → '), false]] : []),
  ];
  const body = v_flex().id('roles-detail').flex_1().min_h(0).overflow_y_scrollbar().px(12).pt(12).pb(12).gap(10);
  const meta = v_flex().gap(6);
  for (const [label, value, isMono] of fields) {
    const val = div().flex_1().min_w(0).text_size(12).text_color(t.foreground).child(value);
    meta.child(h_flex().gap(10).items_start().min_w(0)
      .child(div().w(68).flex_shrink_0().text_size(11.5).text_color(t.muted).child(label))
      .child(isMono ? mono(val) : val));
  }
  body.child(meta);
  body.child(div().h(1).bg(t.border));
  body.child(v_flex().gap(6)
    .child(div().text_size(11.5).font_semibold().text_color(t.muted).child('Prompt'))
    .child(div().px(10).py(8).rounded(6).bg(t.raised).text_size(12.5).line_height('18px')
      .text_color(role.body ? t.foreground : t.muted)
      .child(role.body || '(empty prompt)')));
  const root = v_flex().flex_1().min_h(0).child(topbar);
  const banner = roleNoticeBar(view, t);
  if (banner) root.child(banner);
  return root.child(body);
}
function roleFieldRow(label, control, t, hint = '') {
  const wrap = v_flex().gap(4).min_w(0);
  const head = h_flex().gap(8).items_center()
    .child(div().text_size(11.5).font_semibold().text_color(t.muted).child(label));
  if (hint) head.child(div().text_size(11).text_color(t.muted).child(hint));
  return wrap.child(head).child(control);
}
function roleInputBox(state, t, isMono = false) {
  const input = Input.new(state).w_full();
  return h_flex().h(28).px(8).items_center().rounded(4).border(1).border_color(t.border).bg(t.input)
    .child(isMono ? mono(input) : input);
}
function roleForm(view, t) {
  const isCreate = view.rolesMode === 'create';
  const existing = !isCreate ? view.roleByName(view.selectedRole) : null;
  const saving = !!view.store.savingRole;
  const nameValue = isCreate ? (view.roleFields.name.value?.() || '').trim() : (view.selectedRole || '');
  const titleText = isCreate ? (view.roleFrom ? `New role · from ${view.roleFrom}` : 'New role') : `Edit ${view.selectedRole}`;
  const subtitle = !isCreate && existing?.source === 'builtin' ? 'Saves as override' : '';
  const titleRow = h_flex().gap(8).items_center().min_w(0)
    .child(div().font_semibold().truncate().child(titleText));
  if (subtitle) titleRow.child(div().text_size(11).text_color(t.working).child(subtitle));
  const topbar = h_flex().gap(8).items_center().px(12).pt(10).pb(6).border_b(1).border_color(t.border)
    .child(div().flex_1().min_w(0).child(titleRow))
    .child(button('role-form-cancel', 'Cancel', cx => view.rolesBack(cx), t, {height: 26, disabled: saving}))
    .child(button('role-form-save', saving ? 'Saving…' : isCreate ? 'Create' : 'Save',
      cx => view.saveRole(cx), t, {primary: true, height: 26, disabled: saving || !nameValue}));
  const form = v_flex().id('roles-form').flex_1().min_h(0).overflow_y_scrollbar().px(12).pt(12).pb(12).gap(10);
  if (isCreate) {
    const templateMenu = v_flex().id('role-from-menu').role('menu').w(220).max_h(260).overflow_y_scrollbar()
      .p(4).gap(1).rounded(6).border(1).border_color(t.border).bg(t.raised).shadow_lg();
    for (const r of (view.store.roles || [])) {
      templateMenu.child(Button.new(`role-from-item-${r.name}`).role('menu_item').w_full().h(26).px(8).rounded(4).justify_start()
        .text_size(12).text_color(t.foreground).hover(s => s.bg(t.hover)).accessibility_label(`Copy from ${r.name}`)
        .on_click((_e, cx) => { cx.stop_propagation(); view.applyRoleTemplate(r.name, cx); })
        .child(h_flex().w_full().gap(6).items_center().min_w(0)
          .child(div().flex_1().min_w(0).truncate().child(r.name))
          .child(div().text_size(10.5).text_color(t.muted).child(r.harness))));
    }
    const startFrom = h_flex().gap(6).items_center()
      .child(button('role-from-blank', 'Blank', cx => view.applyRoleTemplate('', cx), t,
        {height: 24, size: 11.5, primary: !view.roleFrom}))
      .child(Popover.new('role-from-popover').anchor('top_left').open(!!view.fromMenuOpen)
        .on_open_change((open, cx) => { view.fromMenuOpen = open; cx.notify(); })
        .trigger(Button.new('role-from-trigger').h(24).px(10).rounded(4).border(1)
          .border_color(view.roleFrom ? t.accent : t.border).bg(view.roleFrom ? t.hover : t.popover)
          .text_size(11.5).text_color(t.foreground).hover(s => s.bg(t.hover))
          .accessibility_label('Copy from existing role')
          .child(view.roleFrom ? `From: ${view.roleFrom} ▾` : 'Copy from… ▾'))
        .content(templateMenu));
    form.child(roleFieldRow('Start from', startFrom, t));
    form.child(roleFieldRow('Name', roleInputBox(view.roleFields.name, t, true), t, 'letters, digits, - or _'));
  }
  const currentHarness = (view.roleFields.harness.value?.() || '').trim();
  const harnessChips = h_flex().gap(4).flex_wrap();
  for (const h of ROLE_HARNESSES) {
    harnessChips.child(button(`role-harness-${h}`, h, cx => view.pickRoleHarness(h, cx), t,
      {height: 22, px: 7, size: 11, primary: currentHarness === h}));
  }
  form.child(roleFieldRow('Harness', v_flex().gap(4).child(roleInputBox(view.roleFields.harness, t, true)).child(harnessChips), t));
  form.child(h_flex().gap(10).min_w(0)
    .child(div().flex_1().min_w(0).child(roleFieldRow('Model', roleInputBox(view.roleFields.model, t, true), t)))
    .child(div().w(180).flex_shrink_0().child(roleFieldRow('Effort', roleInputBox(view.roleFields.effort, t, true), t))));
  const currentSession = (view.roleFields.session.value?.() || '').trim();
  const sessionChips = h_flex().gap(4).flex_wrap();
  for (const s of ROLE_SESSIONS) {
    sessionChips.child(button(`role-session-${s}`, s, cx => { view.roleFields.session.set_value(s); cx.notify(); }, t,
      {height: 22, px: 7, size: 11, primary: currentSession === s}));
  }
  form.child(roleFieldRow('Session', v_flex().gap(4).child(roleInputBox(view.roleFields.session, t, true)).child(sessionChips), t));
  form.child(roleFieldRow('Start command', roleInputBox(view.roleFields.start, t, true), t, 'must include {brief}'));
  form.child(roleFieldRow('Resume command', roleInputBox(view.roleFields.resume, t, true), t, 'optional'));
  form.child(roleFieldRow('Trust flags', roleInputBox(view.roleFields.trust, t, true), t, 'optional'));
  form.child(roleFieldRow('Prompt', div().px(8).py(6).rounded(4).border(1).border_color(t.border).bg(t.input)
    .child(Textarea.new(view.roleFields.body).w_full().text_size(12.5)), t));
  const root = v_flex().flex_1().min_h(0).child(topbar);
  const banner = roleNoticeBar(view, t);
  if (banner) root.child(banner);
  return root.child(form);
}
function rolesView(view, t) {
  if (view.rolesMode === 'detail') return roleDetail(view, t);
  if (view.rolesMode === 'edit' || view.rolesMode === 'create') return roleForm(view, t);
  return roleList(view, t);
}
export function renderRail(view) {
  const t = palette(), vm = view.model(), isJournal = view.page === 'journal', isRoles = view.page === 'roles';
  const state = vm.working ? `Working · ${vm.elapsed}` : vm.status === 'off' ? 'Off' : vm.available ? 'Idle' : 'Degraded';
  const header = h_flex().h(36).flex_shrink_0().px(12).gap(8).items_center().border_b(1).border_color(t.border);
  if (isJournal) header.child(button('journal-back','←',cx => { view.page='stream'; view.follow=true; view.latest(cx); },t,{px:4,label:'Back to stream'}).border(0));
  if (isRoles) header.child(button('roles-back','←',cx => view.rolesBack(cx),t,{px:4,label:view.rolesMode === 'list' ? 'Back to stream' : 'Back to roles'}).border(0));
  header.child(div().font_semibold().child(isJournal ? 'Journal' : isRoles ? 'Roles' : 'Coordinator'));
  if (!isJournal && !isRoles) header.child(div().text_size(12).text_color(vm.working ? t.working : t.muted).child(state));
  header.child(div().flex_1()).child(Popover.new('rail-actions').anchor('top_right').open(view.menuOpen)
    .on_open_change((open,cx) => { view.menuOpen=open; if (!open) view.confirmRestart=false; cx.notify(); })
    .trigger(Button.new('rail-more').h(24).w(26).track_focus(view.menuFocus).accessibility_label('Coordinator menu').child('⋯'))
    .content(menu(view,vm,t)))
    .child(button('rail-close','×',() => view.dismiss(),t,{px:6,label:'Close coordinator'}).border(0));
  const root = v_flex().id('rail-dialog').relative().size_full().min_h(0).min_w(0).bg(t.popover).text_color(t.foreground).text_size(13)
    .role('dialog').accessibility_label('Coordinator').track_focus(view.rootFocus).on_key_down((event,cx) => view.key(event,cx)).child(header);
  if (isJournal) return root.child(journal(view,t));
  if (isRoles) return root.child(rolesView(view,t));
  if (vm.needsYou?.length) root.child(div().px(12).pt(8).child(needs(view,vm,t)));
  const body = v_flex().id('rail-log').flex_1().min_h(0).min_w(0).overflow_y_scrollbar().track_scroll(view.scroll)
    .track_focus(view.logFocus).tab_index(1).role('log').accessibility_label('Coordinator stream').px(12).pt(12)
    .on_size_change((size,cx) => { if (view.logHeight !== size.height) { view.logHeight=size.height; if (view.follow) view.latest(cx); else cx.notify(); } })
    .on_scroll_wheel((event,cx) => view.scrolled(event.delta.y,cx));
  let children = 0;
  const add = element => { body.child(element); children++; };
  if (view.windowStart > 0) add(button('rail-older','Show older entries',cx => view.older(cx),t,{size:11}));
  if (!vm.rows.length) add(div().text_color(t.muted).child('Nothing in flight. Ask for something below.'));
  vm.rows.slice(view.windowStart,view.windowStart+100).forEach(row => add(entry(view,row,t)));
  if (view.store.pending && view.follow) add(entry(view,view.store.pending,t));
  // Revealing this last child is how the stream scrolls to the bottom.
  add(div().id('rail-end').h(1).flex_shrink_0());
  if (view.pinBottom) { view.pinBottom = false; view.scroll.scroll_to_item(children-1); }
  root.child(body);
  if (!view.follow || view.windowStart+100 < vm.rows.length) root.child(h_flex().justify_center().py(4)
    .child(button('rail-latest','Jump to latest ↓',cx => view.latest(cx),t,{size:11,pill:true,color:t.muted})));
  if (vm.working) {
    const dot = div().size(5).rounded(3).bg(t.working);
    if (!view.reducedMotion) dot.loop('opacity',{from:.3,to:1,period:1200});
    root.child(h_flex().px(12).gap(8).h(22).items_center().text_size(12).text_color(t.muted)
      .child(dot).child(div().flex_1().child(vm.step)).child(div().child(vm.elapsed)));
  }
  const composer = v_flex().flex_shrink_0().gap(6).px(12).pb(12);
  if (view.question) composer.child(h_flex().gap(8).items_center().min_w(0).text_size(11.5).text_color(t.muted)
    .child(div().min_w(0).flex_1().truncate().child(`Answering: ${view.question.text || view.question.time}`))
    .child(button('rail-context-clear','×',cx => { view.question = null; view.input.focus(); cx.notify(); },t,{px:5,label:'Clear reply context'}).border(0)));
  if (vm.status === 'off') composer.child(h_flex().h(36).px(10).items_center().rounded(6).border(1).border_dashed().border_color(t.border)
    .child(div().flex_1().text_color(t.muted).child('Coordinator is off'))
    .child(button('rail-start','Start',cx => view.coordinatorAction('start-coordinator',cx),t,{color:t.accent})));
  else {
    const enabled = vm.canSend && !view.store.sending && !!view.draft.trim();
    const line = h_flex().min_h(36).gap(8).items_end().pl(12).pr(5).py(3).min_w(0).rounded(6).border(1)
      .border_color(view.draft ? t.accent : t.border).bg(t.input)
      .child(mono(div().flex_shrink_0().h(26).font_semibold().text_size(15).text_color(vm.working ? t.working : t.accent).child('›')));
    line.child(h_flex().flex_1().min_w(0).items_center().tab_index(4).child(Textarea.new(view.input).w_full().text_size(13)));
    line.child(Button.new('rail-send').tab_index(5).size(26).rounded(4).flex_shrink_0()
      .disabled(!enabled).text_color(enabled ? t.accent : t.muted).hover(s => s.bg(t.hover))
      .accessibility_label('Send message').tooltip('Send message')
      .on_click((_e,cx) => { cx.stop_propagation(); view.store.send(cx,view); })
      .child(svg(`mods/${MOD_NAME}/send.svg`).size(16).text_color(enabled ? t.accent : t.muted)));
    composer.child(line);
  }
  const notice = view.store.noticeUntil && Date.now() > view.store.noticeUntil ? '' : view.store.notice;
  if (notice || view.store.error) composer.child(div().text_size(11).text_color(view.store.error ? t.blocked : t.muted).child(view.store.error || notice));
  return root.child(composer);
}
