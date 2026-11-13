import { div, svg } from 'gpui-kit';
import { h_flex, Button } from 'gpui-base';
import { theme } from 'herdr';
import { MOD_NAME } from './runtime.js';
import { statusbarModel } from './model.js';

export function renderStatus(view, cx) {
  const t = theme(), store = view.store;
  const vm = statusbarModel(store.status, { width: view.width, now: Date.now(), starting: store.starting, failed: store.failed, failures: store.failures, lastOkMs: store.status?.generated_at_ms, error: store.error });
  const icon = svg(`mods/${MOD_NAME}/${vm.icon}`).size(13);
  if (vm.state === 'starting') icon.spin(900);
  const content = h_flex().min_w(0).gap(6).items_center().overflow_hidden()
    .child(div().text_color(t[vm.iconRole] || t.dim).child(icon));
  if (vm.tier === 'full') {
    const text = h_flex().min_w(0).gap(0).items_center().overflow_hidden();
    if (vm.state === 'running' && vm.segments?.length) vm.segments.forEach((s, i) => {
      if (i) text.child(div().text_color(t.dim).child(' · '));
      text.child(div().min_w(0).truncate().text_color(t[s.role] || t.muted).child(s.text));
    });
    else if (vm.state === 'degraded' && store.status?.counts?.needs_you) {
      const suffix = ` · ${store.status.counts.needs_you} needs you${vm.stale ? ' ~' : ''}`;
      text.child(div().min_w(0).truncate().text_color(t.working).child(vm.text.slice(0, -suffix.length)))
        .child(div().min_w(0).truncate().text_color(t.blocked).child(suffix));
    } else text.child(div().min_w(0).truncate().text_color(vm.state === 'degraded' ? t.working : t.dim).child(vm.text));
    if (vm.state === 'running' && vm.segments?.length && vm.stale) text.child(div().text_color(t.dim).child(' ~'));
    content.child(text);
  } else if (vm.tier === 'dots') {
    if (vm.state === 'running' && vm.segments?.length) {
      for (const s of vm.segments) content.child(h_flex().gap(3).items_center().child(div().size(6).rounded(3).bg(t[s.role === 'blocked' ? 'blocked' : s.key === 'running' ? 'working' : 'dim'])).child(div().text_color(t.muted).child(s.compact)));
      if (vm.stale) content.child(div().text_color(t.dim).child('~'));
    } else content.child(div().min_w(0).truncate().text_color(t.dim).child(vm.compactText));
  } else if (store.status?.counts?.needs_you > 0) content.child(div().size(6).rounded(3).bg(t.blocked));
  const button = Button.new('coord-readout').track_focus(view.focus).tab_index(0).h(26).px(6).min_w(0).rounded(3).border(1)
    .border_color(view.focus.is_focused() ? t.accent : t.chrome).hover(s => s.bg(t.hover).text_color(t.foreground))
    .tooltip(vm.tooltip).accessibility_label(vm.label || vm.tooltip)
    .on_click((_e, cx) => { if (vm.click === 'board') view.openBoard('board'); else if (vm.click === 'up') view.start(cx); else if (vm.click === 'reread') view.refresh(cx); })
    .on_mouse_down('right', (_e, cx) => { cx.stop_propagation(); view.openBoard('board'); }).child(content);
  return h_flex().size_full().px(10).items_center().bg(t.chrome).border_t(1).border_color(t.border).text_size(11.8).text_color(t.muted).child(button);
}
