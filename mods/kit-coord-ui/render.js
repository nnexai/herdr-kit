import { div, svg } from 'gpui-kit';
import { h_flex, v_flex, Button, Input, TextView, Popover } from 'gpui-base';
import { theme, settings } from 'herdr';
import { MOD_NAME } from './runtime.js';
import { clock } from './model.js';

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
function colors(t) {
  return {person:t.foreground,reply:t.accent,placed:settings().dark === false ? t.ansi6 : '#6cc4c9',
    report:t.done,progress:t.working,question:t.blocked,decision:settings().dark === false ? t.ansi5 : '#b79ae6',neutral:t.muted};
}
function bead(row, width, t, view, color) {
  const rail = div().relative().w(width).flex_shrink_0().self_stretch();
  rail.child(div().absolute().left(width/2-1).top(15).bottom(-27).w(2).bg(tint(color,'80')));
  rail.child(div().absolute().left(width/2+7).top(15).w(width/2-7).h(1).bg(tint(color,'80')));
  if (row.kind === 'person') return rail.child(svg(`mods/${MOD_NAME}/person.svg`).absolute().left(width/2-7).top(8).size(14).text_color(color));
  const big = row.kind === 'reply', question = row.kind === 'question' && !row.answer;
  const size = big ? 14 : question ? 12 : 8;
  if (big || question) {
    const halo = div().absolute().left(width/2-(size+10)/2).top(15-(size+10)/2).size(size+10).rounded(20)
      .bg(tint(color,'18')).border(1).border_color(tint(color,'80'));
    if (question && !view.reducedMotion) halo.loop('opacity',{from:.3,to:.9,period:1900});
    rail.child(halo);
  }
  const node = div().absolute().left(width/2-size/2).top(15-size/2).size(size)
    .rounded(row.kind === 'decision' ? 2 : size/2).border(row.kind === 'placed' ? 2 : 1).border_color(color)
    .bg(row.kind === 'placed' ? t.canvas : color);
  return rail.child(node);
}
function chip(view, row, link, index, t) {
  return Button.new(`link-${row.key}-${index}`).tab_index(3).h(20).px(8).rounded(10).border(1).border_dashed()
    .border_color(tint(t.accent,'80')).bg(t.canvas).text_size(11).max_w(view.width < 520 ? 180 : 320)
    .hover(s => s.bg(tint(t.accent,'24'))).tooltip(link.label).accessibility_label(`${link.kind === 'terminal' ? 'tab' : 'space'} ${link.label}`)
    .on_click((_event,cx) => { cx.stop_propagation(); view.focusLink(link,cx); })
    .child(h_flex().min_w(0).gap(5).child(mono(div().flex_shrink_0().text_color(t.muted).child(link.kind === 'terminal' ? 'tab' : 'space')))
      .child(mono(div().min_w(0).truncate().text_color(t.accent).child(link.label))));
}
function entry(view, row, t) {
  const narrow = view.width < 520, color = row.answer ? t.done : colors(t)[row.kind];
  const reply = row.kind === 'reply', question = row.kind === 'question' && !row.answer;
  const card = v_flex().min_w(0).flex_1().gap(3).px(reply || question ? 12 : 10).pt(6).pb(reply || question ? 10 : 7)
    .rounded(6).border(1).border_color(question ? tint(color,'70') : reply ? t.selected : tint(t.border,'55'))
    .bg(question ? tint(color,'12') : reply ? t.raised : row.kind === 'person' ? tint(t.foreground,'0c') : t.canvas);
  const header = h_flex().min_w(0).gap(8).items_center()
    .child(mono(div().flex_shrink_0().text_size(10).font_semibold().text_color(color).child(row.answer ? 'ANSWERED' : row.label.toUpperCase())));
  if (row.scope) header.child(mono(div().min_w(0).flex_1().text_right().truncate().text_size(10.5).text_color(t.muted).child(row.scope)));
  card.child(header);
  if (row.answer) card.child(div().text_size(12).text_color(t.muted).child(`↳ you answered: ${row.answer}`));
  else if (reply) card.child(TextView.markdown(`body-${row.key}`,row.text).selectable(true).scrollable(false).text_size(13.5).line_height(1.55));
  else if (row.kind === 'person') card.child(h_flex().gap(8).items_start()
    .child(mono(div().text_color(t.accent).font_semibold().child('›')))
    .child(div().min_w(0).flex_1().child(row.text)));
  else if (row.kind === 'decision') card.child(div().border_l(2).border_color(tint(color,'99')).pl(8).text_size(12.5).child(row.text));
  else card.child(div().min_w(0).text_size(12.5).line_height(1.5).child(row.text));
  if (row.detail && !row.answer) card.child(div().text_size(12).text_color(t.muted).line_height(1.4).child(row.detail));
  if (row.meta?.length) card.child(mono(div().text_size(11).text_color(t.muted).child(row.meta.join('  /  '))));
  if (row.links?.length) {
    const chips = h_flex().gap(6).mt(3).flex_wrap().min_w(0);
    row.links.forEach((link,index) => chips.child(chip(view,row,link,index,t)));
    card.child(chips);
  }
  if (question && row.options?.length) {
    const options = h_flex().gap(8).mt(7).flex_wrap();
    row.options.forEach((option,index) => options.child(button(`reply-${row.key}-${index}`,option,cx => view.chooseReply(row,option,cx),t,
      {height:26,pill:true,color:t.accent,bg:t.canvas,primary:view.question?.id === row.questionId && view.draft === option})
      .max_w(Math.max(100,view.width - 132)).truncate().tooltip(option)));
    card.child(options).child(div().mt(3).text_size(11).text_color(t.muted).child('Fills the composer — press Enter to send.'));
  }
  const line = h_flex().id(row.key).relative().items_start().min_w(0).max_w(780).flex_shrink_0().pb(12).role('article')
    .on_size_change((size,cx) => { if (view.rowHeights.get(row.key) !== size.height) { view.rowHeights.set(row.key,size.height); cx.notify(); } })
    .accessibility_label(`${clock(row.ts_ms)} ${row.label}: ${row.text}`)
    .child(mono(div().w(narrow ? 38 : 50).flex_shrink_0().pt(6).pr(narrow ? 6 : 8).whitespace_nowrap().text_right().text_size(narrow ? 10.5 : 11).text_color(t.gutter).child(clock(row.ts_ms))))
    .child(bead(row,narrow ? 24 : 30,t,view,color)).child(card);
  return line;
}
function head(view, vm, t) {
  const narrow = view.width < 520, width = narrow ? 24 : 30, color = vm.working ? t.working : t.muted;
  const node = div().relative().w(width).h(44).flex_shrink_0();
  if (vm.working) {
    const flow = v_flex().absolute().w(2).h(135).top(-120).left(width/2-1);
    for (const alpha of ['00','0c','19','26','40','59','80','b3','cc','80']) flow.child(div().w(2).h(13.5).flex_shrink_0().bg(tint(color,alpha)));
    if (!view.reducedMotion) flow.loop('opacity',{from:.45,to:1,period:3200});
    node.child(flow).child(div().absolute().left(width/2-10).top(5).size(20).rounded(10).bg(tint(color,'18')))
      .child(div().absolute().left(width/2-7).top(8).size(14).rounded(7).bg(tint(color,'30')));
    if (view.reducedMotion) node.child(div().absolute().left(width/2-11).top(4).size(22).rounded(11).border(2).border_color(tint(color,'b0')));
    else node.child(svg(`mods/${MOD_NAME}/orbit.svg`).absolute().left(width/2-14).top(1).size(28).text_color(color).spin(2400));
  }
  node.child(div().absolute().left(width/2-4).top(11).size(8).rounded(4).border(2).border_color(color).bg(vm.working ? color : t.canvas));
  const title = h_flex().gap(4).items_center().min_w(0).child(div().min_w(0).text_size(12.5).text_color(color).child(vm.head));
  if (vm.working) {
    const caret = div().w(6).h(11).flex_shrink_0().bg(color);
    if (!view.reducedMotion) caret.loop('opacity',{from:.2,to:.9,period:1100});
    title.child(caret);
  }
  return h_flex().id('rail-head').items_start().min_w(0).max_w(780).flex_shrink_0().min_h(44)
    .child(mono(div().w(narrow ? 38 : 50).flex_shrink_0().pt(6).pr(8).text_size(11).text_right().text_color(color).child(vm.elapsed)))
    .child(node).child(v_flex().min_w(0).flex_1().pt(5).gap(3).child(title)
      .child(mono(div().min_w(0).truncate().text_size(11).text_color(t.muted).tooltip(vm.step).child(vm.working ? `› ${vm.step}` : vm.step))));
}
function menu(view, vm, t) {
  const panel = v_flex().id('rail-menu').role('menu').w(232)
    .p(5).gap(3).rounded(6).border(1).border_color(t.border).bg(t.raised).shadow_lg();
  const item = (id,label,action,hint = '',danger = false) => Button.new(id).tab_index(1).w_full().h(28).px(8).rounded(4)
    .bg(t.raised).text_size(12.5).text_color(danger ? t.blocked : t.foreground).justify_between()
    .hover(s => s.bg(t.hover)).role('menu_item').accessibility_label(label)
    .on_click((_event,cx) => { cx.stop_propagation(); action(cx); }).child(label)
    .child(div().text_size(11).text_color(t.muted).child(hint));
  if (vm.available) {
    panel.child(item('rail-clear','Clear context',cx => view.coordinatorAction('clear-context',cx),'new session'));
    panel.child(item('rail-restart','Restart',cx => view.coordinatorAction('restart',cx),'when idle'));
    if (view.store.status?.coordinator?.restart_pending) {
      panel.child(div().h(1).my(3).bg(t.border));
      if (view.confirmRestart) panel.child(div().p(8).text_size(12).text_color(t.muted).child('Restart now interrupts the coordinator mid-turn.'))
        .child(item('rail-force-confirm','Confirm restart now',cx => view.coordinatorAction('restart-now',cx),'',true))
        .child(item('rail-force-cancel','Cancel',cx => { view.confirmRestart = false; cx.notify(); }));
      else panel.child(item('rail-force','Restart now…',cx => { view.confirmRestart = true; cx.notify(); },'interrupts turn',true));
    }
  }
  if (view.store.status?.coordinator?.terminal) panel.child(item('rail-show','Show coordinator',cx => view.showCoordinator(cx),'raw terminal'));
  panel.child(div().h(1).my(3).bg(t.border));
  panel.child(item('rail-motion','Reduce motion',cx => view.toggleMotion(cx),view.reducedMotion ? 'On' : 'Off'));
  return panel;
}
export function renderRail(view) {
  const t = palette(), vm = view.model(), narrow = view.width < 520;
  const title = h_flex().gap(8).items_center().flex_shrink_0()
    .child(svg(`mods/${MOD_NAME}/rail.svg`).w(18).h(24).text_color(t.accent))
    .child(div().font_semibold().text_size(15).child('Coordinator'));
  if (!narrow) title.child(mono(div().text_size(11).text_color(t.muted).child('mission log')));
  const status = h_flex().gap(8).items_center().min_w(0).text_size(11.5).text_color(t.muted)
    .child(h_flex().gap(5).items_center().text_color(vm.available ? t.accent : t.muted)
      .child(div().size(6).rounded(3).bg(vm.available ? t.accent : t.muted)).child(vm.status));
  if (vm.model) status.child(div().max_w(narrow ? 120 : 180).truncate().tooltip(vm.model).child(vm.model));
  status.child(div().min_w(0).truncate().child(`${vm.active} active${narrow ? '' : ' workstreams'}`));
  const header = v_flex().relative().flex_shrink_0().gap(6);
  const top = h_flex().gap(10).items_center().min_w(0).child(title);
  if (!narrow) top.child(status);
  top.child(div().flex_1().min_w(0)).child(Popover.new('rail-actions').open(view.menuOpen)
    .on_open_change((open,cx) => { view.menuOpen = open; if (!open) view.confirmRestart = false; cx.notify(); })
    .trigger(Button.new('rail-more').tab_index(0).h(24).w(26).rounded(4).border(1).border_color(t.border)
      .bg(t.popover).text_color(t.muted).hover(s => s.bg(t.hover)).track_focus(view.menuFocus)
      .accessibility_label('Coordinator menu').child('⋯'))
    .content(menu(view,vm,t)));
  header.child(top);
  if (narrow) header.child(status);
  const rows = vm.rows.slice(view.windowStart,view.windowStart + 100);
  const body = v_flex().id('rail-log').relative().flex_1().min_h(0).min_w(0).rounded(6).border(1).border_color(tint(t.border,'80')).bg(t.canvas)
    .overflow_y_scrollbar().track_scroll(view.scroll).track_focus(view.logFocus).tab_index(1).role('log').accessibility_label('Coordinator mission log')
    .px(narrow ? 8 : 10).pt(18).on_size_change((size,cx) => {
      if (view.logHeight === size.height) return;
      view.logHeight = size.height; if (view.follow) view.latest(cx); else cx.notify();
    })
    .on_scroll_wheel((event,cx) => view.scrolled(event.delta.y,cx));
  if (view.windowStart > 0) body.child(button('rail-older','Show older entries',cx => view.older(cx),t,{size:11}));
  if (!vm.rows.length) body.child(v_flex().min_h(140).justify_center().pl(narrow ? 62 : 80).gap(8)
    .child(div().text_size(16).font_semibold().text_color(t.foreground).child(view.store.error ? 'Coordinator is unavailable' : vm.available ? 'Your mission log starts here' : 'A quiet place to coordinate'))
    .child(div().text_color(t.muted).max_w(400).line_height(1.5).child(view.store.error ? 'kit-coord may be missing or unavailable. Open Board or the raw coordinator; your draft is kept.' : vm.available ? 'Tell the coordinator what you need. Its replies and key events will meet on this rail.' : 'Start the coordinator from Board. Its terminal and the existing Timeline remain available.'))
    .child(h_flex().child(button('rail-empty-board','Open Board',cx => view.coordinatorAction('show',cx),t,{color:t.accent}))));
  rows.forEach(row => body.child(entry(view,row,t)));
  body.child(head(view,vm,t));
  body.child(div().h(20).flex_shrink_0());
  const logWrap = div().relative().flex_1().min_h(0).min_w(0).child(body.size_full());
  const composer = v_flex().flex_shrink_0().gap(6);
  if (view.question) composer.child(h_flex().gap(8).items_center().min_w(0).text_size(11.5).text_color(t.muted)
    .child(div().min_w(0).flex_1().truncate().child(`↳ replying to question · ${view.question.time}`))
    .child(button('rail-context-clear','×',cx => { view.question = null; view.input.focus(); cx.notify(); },t,{px:5,label:'Clear reply context'}).border(0)));
  const enabled = vm.canSend && !view.store.sending && !!view.draft.trim();
  const line = h_flex().h(36).gap(8).items_center().pl(12).pr(5).min_w(0).rounded(6).border(1)
    .border_color(view.draft ? t.accent : t.border).bg(t.input)
    .child(mono(div().flex_shrink_0().font_semibold().text_size(15).text_color(vm.working ? t.working : t.accent).child('›')));
  line.child(div().flex_1().min_w(0).h(30).overflow_hidden().child(Input.new(view.input).tab_index(4).text_size(13)));
  line.child(button('rail-send',narrow ? 'Send' : 'Send ↵',cx => view.store.send(cx,view),t,{disabled:!enabled,primary:enabled,height:26,tabIndex:5,color:enabled ? undefined : t.muted}));
  composer.child(line);
  const footer = h_flex().flex_shrink_0().gap(12).min_w(0).text_size(11).text_color(t.muted)
    .child(div().flex_1().min_w(0).truncate().tooltip(view.store.notice || 'Enter send · Esc close')
      .child(view.store.notice || h_flex().gap(3).items_center()
        .child(mono(div().px(3).rounded(3).border(1).border_color(t.border).text_size(10.5).child('Enter'))).child('send ·')
        .child(mono(div().px(3).rounded(3).border(1).border_color(t.border).text_size(10.5).child('Esc'))).child('close')));
  if (!narrow) footer.child(div().flex_shrink_0().child(`journal · synced ${vm.sync}`));
  const root = v_flex().id('rail-dialog').relative().size_full().min_h(0).min_w(0).px(16).pt(14).pb(12).gap(10)
    .bg(t.popover).text_color(t.foreground).text_size(13).role('dialog').accessibility_label('Coordinator Mission Log')
    .track_focus(view.rootFocus).on_key_down((event,cx) => view.key(event,cx)).child(header);
  if (view.questionOffscreen(vm)) logWrap.child(h_flex().absolute().left(0).right(0).bottom(12).justify_center()
    .child(div().rounded(13).bg('#2c1f20').shadow_md()
      .child(button('rail-question-jump',`↑ ${vm.questions.length} question${vm.questions.length === 1 ? '' : 's'} waiting`,cx => view.jumpQuestion(cx),t,{pill:true,height:26,color:t.blocked,bg:'#2c1f20'}))));
  else if (vm.rows.length && (!view.follow || view.windowStart + 100 < vm.rows.length)) logWrap.child(h_flex().absolute().left(0).right(0).bottom(12).justify_center()
    .child(div().rounded(13).bg(t.popover).shadow_md()
      .child(button('rail-latest','Back to live',cx => view.latest(cx),t,{pill:true,height:24,color:t.accent}))));
  root.child(logWrap).child(composer).child(footer);
  return root;
}
