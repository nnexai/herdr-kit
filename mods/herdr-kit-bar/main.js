import { View, div, svg } from 'gpui-kit';
import { h_flex, Button } from 'gpui-base';
import { agents, spaces, theme, props, shared, run, exec } from 'herdr';
import { MOD_NAME } from './runtime.js';

// Producers own collection, lifecycle and formatting. This view only composes
// their published readouts with host chrome and forwards explicit clicks.
const coordKey = 'herdr-kit.coord.readout.v1';
const usageKey = 'herdr-kit.usage.readout.v1';
let maintaining = false;
function read(key, mod) {
  const value = mod ? shared(key) : null;
  return value?.schema_version === 1 && value.mod === mod ? value : null;
}

export default class KitBar extends View {
  init(_p, cx) {
    this.options = props() ?? {};
    this.width = 1000;
    this.coordFocus = cx.focus_handle();
    this.usageFocus = cx.focus_handle();
    cx.timer.every(3000, cx => cx.notify());
    if (this.options.manager) {
      this.maintain(cx);
      cx.timer.every(21600000, cx => this.maintain(cx));
    }
  }
  maintain(cx) {
    if (maintaining) return;
    maintaining = true;
    cx.spawn(async () => {
      try {
        await exec(this.options.manager, ['--data-dir', this.options.data_dir, 'update', 'maintain'], {timeout:600000});
      } catch (_) {} finally { maintaining = false; }
    });
  }
  coord(t) {
    const vm = read(coordKey, this.options.coord_mod);
    const content = h_flex().min_w(0).gap(6).items_center().overflow_hidden();
    if (vm) {
      const icon = svg(vm.icon_path).size(13);
      if (vm.state === 'starting') icon.spin(900);
      content.child(div().text_color(t[vm.iconRole] || t.dim).child(icon));
      if (vm.tier === 'full') content.child(div().min_w(0).truncate().child(vm.text));
      else if (vm.tier === 'dots') {
        if (vm.state === 'running' && vm.segments?.length) {
          for (const s of vm.segments) content.child(h_flex().gap(3).items_center()
            .child(div().size(6).rounded(3).bg(t[s.role === 'blocked' ? 'blocked' : s.key === 'running' ? 'working' : 'dim']))
            .child(div().text_color(t.muted).child(s.compact)));
          if (vm.stale) content.child(div().text_color(t.dim).child('~'));
        } else content.child(div().min_w(0).truncate().child(vm.compactText));
      }
    } else content.child('…');
    const label = vm?.state === 'running' && this.options.coord_ui_mod
      ? vm.tooltip.replace('Click to focus the coordinator.', 'Click to open the Coordinator dialog.')
      : vm?.tooltip ?? 'Reading coordinator status';
    return Button.new('coord-readout').track_focus(this.coordFocus).tab_index(0)
      .w(80).flex_shrink_0().h(26).px(6).min_w(0).rounded(3).border(1)
      .border_color(this.coordFocus.is_focused() ? t.accent : t.chrome)
      .hover(s => s.bg(t.hover).text_color(t.foreground)).tooltip(label).accessibility_label(label)
      .on_click(() => {
        if (vm?.click === 'none') return;
        if (vm?.state === 'running') {
          if (this.options.coord_ui_mod) run(`${this.options.coord_ui_mod}.show`, {});
          else {
            const terminal = shared('herdr-kit.coord.status.v1')?.coordinator?.terminal;
            if (terminal) run('agent.focus', {terminal});
          }
        } else {
          const command = vm?.click === 'up' ? 'start-coordinator' : 'reread-status';
          run(`${this.options.coord_mod}.${command}`, {});
        }
      })
      .on_mouse_down('right', (_e,cx) => {
        cx.stop_propagation();
        if (this.options.coord_ui_mod) run(`${this.options.coord_ui_mod}.show`, {});
      })
      .child(content);
  }
  usage(t) {
    const vm = read(usageKey, this.options.usage_mod);
    const narrow = this.width < 500;
    const label = vm?.label ?? 'Reading subscription limits';
    const content = h_flex().gap(5).items_center().child('Limits');
    if (!vm || vm.state === 'loading') content.child('…');
    else if (vm.state === 'unavailable') content.child('unavailable');
    else if (vm.provider) {
      if (!narrow) content.child(div().text_color(t.muted).child(vm.provider));
      content.child(div().text_color(t[vm.role] || t.dim).child(vm.amount));
      if (vm.additional) content.child(`+${vm.additional}`);
    }
    return Button.new('usage-readout').track_focus(this.usageFocus).tab_index(0)
      .flex_shrink_0().h(26).px(6).items_center().justify_end().overflow_hidden()
      .bg(t.chrome).rounded(3).border(1)
      .border_color(this.usageFocus.is_focused() ? t.accent : t.chrome)
      .hover(s => s.bg(t.hover).text_color(t.foreground)).text_color(t.dim)
      .tooltip(label).accessibility_label(label)
      .on_click(() => run(`${this.options.usage_mod}.show`, {})).child(content);
  }
  render() {
    const t = theme(), space = spaces().find(s => s.active), list = agents();
    const needs = list.filter(a => a.status === 'blocked').length;
    const statusLabel = (icon,text) => h_flex().min_w(0).gap(6).items_center()
      .child(svg(`mods/${MOD_NAME}/${icon}.svg`).size(13))
      .child(div().flex_1().min_w(0).truncate().child(text));
    const left = h_flex().flex_1().min_w(0).gap(8).items_center().overflow_hidden()
      .child(statusLabel(space?.machine ? 'server' : 'monitor',space?.machine ?? 'This computer').flex_initial())
      .when(Boolean(space?.branch),el => el.child(div().max_w(160).min_w(0).flex_initial().child(statusLabel('branch',space.branch))))
      .child(h_flex().flex_shrink_0().gap(6).items_center()
        .child(div().size(6).rounded(3).bg(t.dim)).child(`${list.length} agents`)
        .when(needs > 0,el => el.child(div().text_color(t.blocked).child(`· ${needs} needs you`))));
    return h_flex().size_full().px(8).gap(8).items_center().bg(t.chrome)
      .border_t(1).border_color(t.border).text_size(11.8).text_color(t.muted)
      .on_size_change((size,cx) => { if (this.width !== size.width) { this.width = size.width; cx.notify(); } })
      .child(left)
      .when(Boolean(this.options.coord_mod), el => el.child(this.coord(t)))
      .when(Boolean(this.options.usage_mod), el => el.child(this.usage(t)));
  }
}
