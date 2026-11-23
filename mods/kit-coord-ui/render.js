import { div, svg, image } from 'gpui-kit';
import { h_flex, v_flex, Button, Checkbox, Textarea, TextView, Popover, Input } from 'gpui-base';
import { theme, settings } from 'herdr';
import { MOD_NAME } from './runtime.js';
import { clock, dayLabel, journalRows, journalFields, filterRoles, roleSummaryLine, ROLE_HARNESSES, ROLE_SESSIONS,
  formatBytes, attachmentKindTag, fitPreviewBox, attachmentActions, shortTaskNum, workstreamAttachments, reportAttachments } from './model.js';

const TRANSPARENT = '#00000000';
const hex6 = color => {
  if (typeof color !== 'string' || !color.startsWith('#')) return null;
  if (color.length === 4) return '#' + [...color.slice(1)].map(c => c + c).join('');
  return color.length >= 7 ? color.slice(0, 7) : null;
};
const tint = (color, alpha) => {
  const base = hex6(color);
  return base ? base + alpha : color;
};
const mix = (a, b, weight) => {
  const ha = hex6(a), hb = hex6(b);
  if (!ha || !hb) return a;
  const ch = (h, i) => parseInt(h.slice(i, i + 2), 16);
  const lerp = i => Math.round(ch(ha, i) * (1 - weight) + ch(hb, i) * weight).toString(16).padStart(2, '0');
  return `#${lerp(1)}${lerp(3)}${lerp(5)}`;
};
const mono = element => element.font_family('JetBrains Mono');
const isNarrow = view => (view.width || 680) < 460;
const pagePad = view => isNarrow(view) ? 12 : 16;

export const palette = () => {
  const t = theme();
  const light = settings().dark === false;
  const fg = t.foreground || (light ? '#1b1b1e' : '#eeeeef');
  const popover = t.popover || (light ? '#ffffff' : '#25272b');
  const rawMuted = t.muted || (light ? '#69696b' : '#98999b');
  // Three-tier ink ramp harmonized with the active Herdr theme (dark or light).
  const secondary = mix(fg, rawMuted, light ? 0.42 : 0.44);
  const muted = mix(rawMuted, fg, 0.14);
  const borderStrong = tint(fg, light ? '28' : '24');
  const card = light
    ? mix(t.raised || '#f6f6f6', popover, 0.2)
    : mix(t.raised || '#2c2e31', fg, 0.03);
  const recessed = light
    ? mix(t.raised || '#f6f6f6', popover, 0.35)
    : mix(popover, t.surface || t.canvas || '#131518', 0.45);
  const input = light
    ? (hex6(t.surface) || popover)
    : mix(popover, t.surface || t.canvas || '#131518', 0.48);
  return light
    ? { ...t, secondary, muted, card, recessed, input, borderStrong, gutter: muted, transparent: TRANSPARENT }
    : { ...t, secondary, muted, card, recessed, input, borderStrong,
        working: '#e5be57', blocked: '#f47b78', done: '#73d29b', gutter: '#8b8f97', transparent: TRANSPARENT };
};

export function button(id, label, handler, t, options = {}) {
  const ghost = !!options.ghost;
  const bg = options.primary ? t.accent : options.bg || (ghost ? t.transparent : t.card);
  const fg = options.primary ? t.accent_foreground : options.color || t.foreground;
  const el = Button.new(id).tab_index(options.tabIndex ?? 3).h(options.height || 26).px(options.px ?? 10)
    .rounded(options.pill ? 13 : 5)
    .border(ghost ? 0 : 1).border_color(options.color || t.borderStrong)
    .bg(bg).text_color(fg).text_size(options.size || 12)
    .disabled(!!options.disabled).flex_shrink_0()
    .hover(s => s.bg(options.primary ? t.accent : t.hover)
      .text_color(options.primary ? t.accent_foreground : options.hoverColor || (ghost ? t.foreground : fg)))
    .accessibility_label(options.label || label)
    .on_click((_e, cx) => { cx.stop_propagation(); handler(cx); }).child(label);
  return el;
}

const age = ms => {
  const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m` : `${Math.floor(s / 3600)}h`;
};

function glyph(state, t, view) {
  const color = state === 'done' ? t.done : state === 'question' ? t.blocked : state === 'blocked' || state === 'running' ? t.working : t.muted;
  if (state === 'running') {
    const ring = svg(`mods/${MOD_NAME}/orbit.svg`).size(14).text_color(color);
    if (!view.reducedMotion) ring.spin(1200);
    return ring;
  }
  return div().w(15).flex_shrink_0().text_color(color).child(({ done: '✓', question: '?', blocked: '!', placed: '○' })[state] || '○');
}

function dayHeaderRow(label, t, pt = 6) {
  return h_flex().gap(10).items_center().pt(pt).pb(4)
    .child(div().text_size(11.5).font_semibold().text_color(t.muted).child(label))
    .child(div().flex_1().h(1).bg(t.border));
}

function menu(view, vm, t) {
  const panel = v_flex().id('rail-menu').role('menu').w(228).p(5).gap(2).rounded(7).border(1).border_color(t.borderStrong).bg(t.card).shadow_lg();
  const item = (id, label, action, color) => panel.child(Button.new(id).role('menu_item').w_full().h(30).px(10).rounded(5).justify_start()
    .text_size(12.5).text_color(color || t.foreground).hover(s => s.bg(t.hover)).accessibility_label(label)
    .on_click((_e, cx) => { cx.stop_propagation(); action(cx); }).child(label));
  const separator = () => panel.child(div().h(1).mx(6).my(3).bg(t.border));
  item('rail-journal', 'Journal', cx => { view.page = 'journal'; view.menuOpen = false; view.journalStart = 0; view.store.journal = view.store.log; cx.notify(); });
  item('rail-roles', 'Roles', cx => view.openRoles(cx));
  if (view.store.status?.coordinator?.terminal) item('rail-show', 'Show raw coordinator', cx => view.showCoordinator(cx));
  if (vm.status !== 'off') {
    separator();
    // Clear context is the graceful restart: a fresh session once the coordinator is idle.
    if (!view.store.status?.coordinator?.restart_pending) item('rail-clear', 'Clear context', cx => view.coordinatorAction('clear-context', cx));
    else if (view.confirmRestart) {
      panel.child(div().px(10).py(6).text_size(12).line_height(1.4).text_color(t.secondary).child('Restart now interrupts the coordinator mid-turn.'));
      item('rail-force-confirm', 'Confirm restart now', cx => view.coordinatorAction('restart-now', cx), t.blocked);
      item('rail-force-cancel', 'Cancel', cx => { view.confirmRestart = false; cx.notify(); });
    } else {
      panel.child(div().px(10).py(6).text_size(12).line_height(1.4).text_color(t.secondary).child('Fresh session requested; it starts when the coordinator is idle.'));
      item('rail-force', 'Restart now…', cx => { view.confirmRestart = true; cx.notify(); });
    }
  }
  separator();
  item('rail-motion', `Reduce motion: ${view.reducedMotion ? 'On' : 'Off'}`, cx => view.toggleMotion(cx));
  return panel;
}

function kindPill(kind, t) {
  const tag = attachmentKindTag(kind);
  const color = kind === 'image' ? t.accent : kind === 'html' ? t.working : (kind === 'log' || kind === 'markdown') ? t.done : t.secondary;
  return mono(h_flex().h(18).px(5).flex_shrink_0().items_center().justify_center().rounded(4).border(1)
    .border_color(tint(color, '4c')).bg(tint(color, '14')).text_size(10.5).font_semibold().text_color(color).child(tag));
}

function attachmentRow(view, att, t, scope = 'trace') {
  const narrow = isNarrow(view);
  const rowKey = `${scope}:${att.id}`;
  const act = attachmentActions(att);
  const preview = view.store.imagePreviews?.get?.(att.id) || null;
  const previewOpen = !!preview;
  const confirmHtml = view.confirmHtmlId === rowKey;
  const hovered = view.hoveredAttachment === rowKey;
  const selected = view.selectedAttachment === rowKey;
  const active = hovered || selected || confirmHtml || previewOpen;
  const sizeText = formatBytes(att.bytes);
  const statusLabel = act.availability === 'missing' ? 'Missing'
    : act.availability === 'pruned' ? 'Pruned'
    : act.availability === 'remote' ? 'Remote'
    : act.availability === 'unsafe' ? 'Blocked'
    : att.mode === 'ref' ? (sizeText ? `${sizeText} · ref` : 'ref')
    : (sizeText ? `${sizeText} · staged` : 'staged');
  const statusColor = !act.available ? t.muted : att.mode === 'ref' ? t.working : t.muted;
  const row = v_flex().id(`att-${rowKey}`).px(8).py(3).rounded(6).gap(4).border(1)
    .border_color(confirmHtml ? tint(t.working, '70') : active ? t.borderStrong : t.transparent)
    .bg(confirmHtml ? tint(t.working, '10') : active ? t.card : t.transparent)
    .on_hover((h, cx) => {
      const next = h ? rowKey : view.hoveredAttachment === rowKey ? null : view.hoveredAttachment;
      if (next !== view.hoveredAttachment) { view.hoveredAttachment = next; cx.notify(); }
    });
  if (act.available && att.resolved_path && !att.machine && typeof row.drag_paths === 'function') {
    row.drag_paths([att.resolved_path]);
  }
  const onRowClick = cx => {
    if (narrow) {
      view.selectedAttachment = selected ? null : rowKey;
      cx.notify();
    } else if (act.canPreview) {
      view.store.toggleImagePreview(cx, view, att);
    } else if (act.canView) {
      view.store.openTextViewer(cx, view, att);
    } else {
      view.selectedAttachment = selected ? null : rowKey;
      cx.notify();
    }
  };
  const top = h_flex().gap(7).items_center().min_w(0);
  const left = h_flex().gap(7).items_center().min_w(0).w_full()
    .child(kindPill(att.kind, t))
    .child(div().flex_1().min_w(0).truncate().text_size(12.5)
      .text_color(act.available ? t.foreground : t.muted).child(att.label || 'attachment'));
  if (!narrow) {
    const prov = [shortTaskNum(att.task), att.ts_ms ? clock(att.ts_ms) : ''].filter(Boolean).join(' · ');
    if (prov) left.child(mono(div().flex_shrink_0().text_size(11).text_color(t.muted).child(prov)));
  }
  top.child(Button.new(`att-select-${rowKey}`).flex_1().min_w(0).h(26).justify_start()
    .accessibility_label(`${att.label}, ${attachmentKindTag(att.kind)}${sizeText ? `, ${sizeText}` : ''}, ${act.availability}`)
    .on_click((_e, cx) => { cx.stop_propagation(); onRowClick(cx); })
    .child(left));

  if (!narrow) {
    const slot = h_flex().w(208).h(22).flex_shrink_0().gap(3).items_center().justify_end().overflow_hidden();
    if (!active) {
      slot.child(mono(div().min_w(0).truncate().text_size(11).text_color(statusColor).child(statusLabel)));
    } else {
      if (act.canPreview) {
        slot.child(button(`att-preview-btn-${rowKey}`, previewOpen ? 'Hide' : 'Preview',
          cx => view.store.toggleImagePreview(cx, view, att), t,
          { size: 11.5, px: 7, height: 22, color: previewOpen ? t.secondary : t.accent, ghost: previewOpen }));
      }
      if (act.canView) {
        slot.child(button(`att-view-btn-${rowKey}`, act.viewLabel,
          cx => view.store.openTextViewer(cx, view, att), t,
          { ghost: true, size: 11.5, px: 6, height: 22, color: t.accent }));
      }
      if (act.canOpen) {
        slot.child(button(`att-open-btn-${rowKey}`, 'Open',
          cx => view.store.openAttachment(cx, view, att), t,
          { ghost: true, size: 11.5, px: 6, height: 22, color: t.secondary }));
      }
      if (act.canBrowser) {
        slot.child(button(`att-browser-btn-${rowKey}`, 'Browser…',
          cx => { view.confirmHtmlId = confirmHtml ? null : rowKey; cx.notify(); }, t,
          { ghost: true, size: 11.5, px: 6, height: 22, color: t.working }));
      }
      if (act.canReveal) {
        slot.child(button(`att-reveal-btn-${rowKey}`, 'Reveal',
          cx => view.store.revealAttachment(cx, view, att), t,
          { ghost: true, size: 11.5, px: 6, height: 22, color: t.secondary }));
      }
      slot.child(button(`att-copy-btn-${rowKey}`, act.available ? 'Copy' : 'Copy path',
        cx => view.store.copyAttachmentPath(cx, view, att), t,
        { ghost: true, size: 11.5, px: 6, height: 22, color: t.secondary }));
    }
    top.child(slot);
  } else {
    const compactStatus = !act.available ? statusLabel : att.mode === 'ref' ? (sizeText ? `${sizeText} · ref` : 'Ref') : sizeText || 'staged';
    top.child(mono(div().flex_shrink_0().text_size(11).text_color(statusColor).child(compactStatus)));
  }
  row.child(top);

  if (narrow && selected) {
    const prov = [shortTaskNum(att.task), att.report_kind, att.ts_ms ? clock(att.ts_ms) : '', att.mode].filter(Boolean).join(' · ');
    if (prov) row.child(mono(div().text_size(11).text_color(t.muted).child(prov)));
    const bar = h_flex().gap(6).flex_wrap().pt(2);
    if (act.canPreview) bar.child(button(`att-n-preview-${rowKey}`, previewOpen ? 'Hide preview' : 'Preview',
      cx => view.store.toggleImagePreview(cx, view, att), t, { size: 11.5, px: 8, height: 24, primary: !previewOpen }));
    if (act.canView) bar.child(button(`att-n-view-${rowKey}`, act.viewLabel,
      cx => view.store.openTextViewer(cx, view, att), t, { size: 11.5, px: 8, height: 24, primary: true }));
    if (act.canOpen) bar.child(button(`att-n-open-${rowKey}`, 'Open',
      cx => view.store.openAttachment(cx, view, att), t, { size: 11.5, px: 8, height: 24 }));
    if (act.canBrowser) bar.child(button(`att-n-browser-${rowKey}`, 'Browser…',
      cx => { view.confirmHtmlId = confirmHtml ? null : rowKey; cx.notify(); }, t, { size: 11.5, px: 8, height: 24, color: t.working }));
    if (act.canReveal) bar.child(button(`att-n-reveal-${rowKey}`, 'Reveal',
      cx => view.store.revealAttachment(cx, view, att), t, { size: 11.5, px: 8, height: 24 }));
    bar.child(button(`att-n-copy-${rowKey}`, 'Copy path',
      cx => view.store.copyAttachmentPath(cx, view, att), t, { size: 11.5, px: 8, height: 24 }));
    row.child(bar);
  }

  if (confirmHtml) {
    row.child(h_flex().gap(8).items_center().flex_wrap().pt(3).border_t(1).border_color(t.border)
      .child(div().flex_1().min_w(140).text_size(11.5).text_color(t.working).child('Opens local HTML in your browser; scripts can run.'))
      .child(button(`att-html-confirm-${rowKey}`, 'Open in browser',
        cx => view.store.openAttachment(cx, view, att, { allowHtml: true }), t,
        { size: 11.5, px: 8, height: 22, color: t.working }))
      .child(button(`att-html-cancel-${rowKey}`, 'Cancel',
        cx => { view.confirmHtmlId = null; cx.notify(); }, t,
        { ghost: true, size: 11.5, px: 6, height: 22, color: t.secondary })));
  }

  if (previewOpen) {
    const box = v_flex().id(`att-preview-box-${rowKey}`).mt(2).p(8).gap(6).rounded(6).border(1).border_color(t.border).bg(t.recessed);
    if (preview.status === 'loading') {
      box.child(div().text_size(11.5).text_color(t.muted).child('Loading inline preview…'));
    } else if (preview.status === 'ready' && preview.relPath) {
      const fit = fitPreviewBox(preview.width, preview.height, narrow ? 220 : 280, narrow ? 132 : 168);
      box.child(div().w(fit.w).h(fit.h).rounded(4).overflow_hidden().border(1).border_color(t.borderStrong).bg(t.popover)
        .child(image(preview.relPath).w(fit.w).h(fit.h)));
      const metaLine = h_flex().gap(6).items_center().flex_wrap()
        .child(mono(div().flex_1().min_w(0).truncate().text_size(11).text_color(t.muted)
          .child(`${String(preview.format || '').toUpperCase()} · ${preview.width}×${preview.height}${preview.bytes ? ` · ${formatBytes(preview.bytes)}` : ''} · ${att.mode}`)));
      if (narrow) {
        metaLine.child(button(`att-prev-hide-${rowKey}`, 'Hide',
          cx => view.store.toggleImagePreview(cx, view, att), t, { ghost: true, size: 11, px: 6, height: 22, color: t.secondary }));
      }
      box.child(metaLine);
    } else {
      box.child(div().text_size(11.5).line_height(1.4).text_color(t.working).child(`Preview unavailable: ${preview.error || 'Cannot render image'}`));
      const fallback = h_flex().gap(6).flex_wrap();
      if (act.canOpen) fallback.child(button(`att-prev-err-open-${rowKey}`, 'Open',
        cx => view.store.openAttachment(cx, view, att), t, { size: 11, px: 7, height: 22 }));
      if (act.canReveal) fallback.child(button(`att-prev-err-reveal-${rowKey}`, 'Reveal',
        cx => view.store.revealAttachment(cx, view, att), t, { size: 11, px: 7, height: 22 }));
      fallback.child(button(`att-prev-err-copy-${rowKey}`, 'Copy path',
        cx => view.store.copyAttachmentPath(cx, view, att), t, { ghost: true, size: 11, px: 6, height: 22, color: t.secondary }));
      fallback.child(button(`att-prev-err-close-${rowKey}`, 'Dismiss',
        cx => { view.store.imagePreviews.delete(att.id); cx.notify(); }, t, { ghost: true, size: 11, px: 6, height: 22, color: t.muted }));
      box.child(fallback);
    }
    row.child(box);
  }

  if (act.availability === 'missing') {
    row.child(div().text_size(11).line_height(1.35).truncate().text_color(t.muted)
      .child(`Missing on disk${att.source_path ? ` · was ${att.source_path}` : ''}`));
  } else if (act.availability === 'pruned') {
    row.child(div().text_size(11).line_height(1.35).text_color(t.muted).child('Pruned · removed from staged store'));
  } else if (act.availability === 'remote') {
    row.child(div().text_size(11).line_height(1.35).text_color(t.muted).child(`Remote host (${att.machine || 'remote'}) · Copy path only`));
  } else if (act.availability === 'unsafe') {
    row.child(div().text_size(11).line_height(1.35).truncate().text_color(t.blocked)
      .child(`Unavailable${att.unsafe_reason ? ` · ${att.unsafe_reason}` : ''}`));
  } else if (att.mode === 'ref') {
    const why = att.reason === 'exceeds_stage_limit' ? 'Not staged (>10 MB limit)'
      : att.reason === 'budget_exceeded' ? 'Not staged (staging budget)'
      : 'Reference only';
    row.child(div().text_size(11).line_height(1.35).truncate().text_color(t.working)
      .child(`${why} · lives at source path`));
  }
  return row;
}

function needs(view, vm, t) {
  const items = vm.needsYou || [], wrap = v_flex().gap(8).flex_shrink_0().max_h(260).overflow_y_scrollbar();
  for (const [index, row] of (view.needsAll ? items : items.slice(0, 2)).entries()) {
    const key = row.questionId || `${row.title}:${row.ts_ms}`, expanded = view.expandedQuestions.has(key);
    const text = row.text || '';
    const card = v_flex().id(`needs-${index}`).gap(6).px(12).py(9).border_l(3).border_color(t.blocked).bg(tint(t.blocked, '12')).rounded(6);
    card.child(h_flex().gap(8).items_center().text_size(12)
      .child(div().font_semibold().text_color(t.blocked).child('Needs you'))
      .child(div().flex_1().min_w(0).truncate().font_medium().text_color(t.foreground).child(row.title || ''))
      .child(mono(div().flex_shrink_0().text_size(11.5).text_color(t.muted).child(age(row.ts_ms)))));
    const content = div().min_w(0).flex_shrink_0().text_size(13.5).line_height('20px').text_color(t.foreground).child(text)
      .on_size_change((size, cx) => {
        const long = size.height > 60, wasLong = view.clampedQuestions.has(key);
        if (long !== wasLong) { if (long) view.clampedQuestions.add(key); else view.clampedQuestions.delete(key); cx.notify(); }
      });
    const textBox = div().min_w(0);
    if (!expanded) textBox.max_h(60).overflow_hidden();
    card.child(textBox.child(content));
    if (view.clampedQuestions.has(key)) card.child(button(`needs-more-text-${index}`, expanded ? 'less' : 'more',
      cx => view.toggle(view.expandedQuestions, key, cx), t, { ghost: true, size: 11.5, px: 4, height: 22, color: t.accent }).self_start());
    if (row.attachment) {
      const att = row.attachment;
      const act = attachmentActions(att);
      const cue = h_flex().gap(7).items_center().min_w(0).px(8).py(4).rounded(5).border(1).border_color(t.border).bg(t.recessed)
        .child(kindPill(att.kind, t))
        .child(div().flex_1().min_w(0).truncate().text_size(12).text_color(t.foreground).child(att.label));
      if (att.bytes) cue.child(mono(div().flex_shrink_0().text_size(11).text_color(t.muted).child(formatBytes(att.bytes))));
      if (act.canView) cue.child(button(`needs-att-view-${index}`, act.viewLabel,
        cx => view.store.openTextViewer(cx, view, att), t, { ghost: true, size: 11.5, px: 6, height: 22, color: t.accent }));
      else if (act.canPreview) cue.child(button(`needs-att-open-${index}`, 'Open',
        cx => view.store.openAttachment(cx, view, att), t, { ghost: true, size: 11.5, px: 6, height: 22, color: t.accent }));
      if (act.canReveal) cue.child(button(`needs-att-reveal-${index}`, 'Reveal',
        cx => view.store.revealAttachment(cx, view, att), t, { ghost: true, size: 11.5, px: 6, height: 22, color: t.secondary }));
      card.child(cue);
    }
    if (row.kind === 'question') {
      const actions = h_flex().gap(6).pt(2).flex_wrap();
      (row.options || []).forEach((option, i) => actions.child(button(`needs-option-${index}-${i}`, option,
        cx => view.chooseReply(row, option, cx), t, { pill: true, height: 28, px: 11, disabled: !vm.canSend || vm.working || view.store.sending })));
      actions.child(button(`needs-reply-${index}`, 'Reply…', cx => view.answer(row, cx), t, { pill: true, height: 28, px: 11 }));
      card.child(actions);
    }
    wrap.child(card);
  }
  if (items.length > 2) wrap.child(button('needs-all', view.needsAll ? 'Show less' : `${items.length - 2} more`,
    cx => { view.needsAll = !view.needsAll; cx.notify(); }, t, { ghost: true, size: 11.5, px: 6, height: 22, color: t.accent }).self_start());
  return wrap;
}

function receipt(view, row, t) {
  const narrow = isNarrow(view);
  const open = view.expandedReceipts.has(row.key), active = open || view.hoveredReceipt === row.key;
  const terminal = (row.links || []).find(link => link.kind === 'terminal');
  const railColor = open ? t.accent
    : row.state === 'question' ? t.blocked
    : row.state === 'blocked' || row.state === 'running' ? tint(t.working, 'b8')
    : row.state === 'done' ? tint(t.done, '8c')
    : t.borderStrong;
  const box = v_flex().id(row.key).ml(narrow ? 6 : 10).pl(12).pr(8).py(5).border_l(2).border_color(railColor)
    .rounded_r(6).bg(open ? t.recessed : active ? t.hover : t.transparent).gap(3)
    .on_hover((hovered, cx) => {
      const next = hovered ? row.key : view.hoveredReceipt === row.key ? null : view.hoveredReceipt;
      if (next !== view.hoveredReceipt) { view.hoveredReceipt = next; cx.notify(); }
    });
  const top = h_flex().gap(8).h(24).items_center().min_w(0);
  top.child(Button.new(`receipt-select-${row.key}`).flex_1().min_w(0).justify_start()
    .accessibility_label(`${open ? 'Hide' : 'Show'} trace for ${row.title || 'workstream'}`)
    .on_click((_e, cx) => { cx.stop_propagation(); view.toggleReceipt(row.key, cx); })
    .child(h_flex().gap(8).items_center().min_w(0).w_full().child(glyph(row.state, t, view))
      .child(div().flex_1().min_w(0).truncate().text_size(13).font_semibold().text_color(t.foreground).child(row.title || 'Workstream'))));
  // One fixed-size slot holds either the meta or the actions, so hovering never moves the row.
  const slot = h_flex().w(narrow ? 132 : 172).h(24).flex_shrink_0().gap(4).items_center().justify_end().overflow_hidden();
  if (!active) {
    const fileCue = row.attachmentCount > 0 ? `${row.attachmentCount} ${row.attachmentCount === 1 ? 'file' : 'files'}` : '';
    const meta = narrow
      ? (fileCue ? [fileCue, row.duration].filter(Boolean).join('  ') : [row.space, row.duration].filter(Boolean).join('  '))
      : [[row.space, fileCue].filter(Boolean).join(' · '), row.duration].filter(Boolean).join('  ');
    slot.child(mono(div().min_w(0).truncate().text_size(11.5).text_color(t.muted).child(meta)));
  } else {
    if (terminal) slot.child(button(`terminal-${row.key}`, narrow ? 'Terminal' : 'Open terminal',
      cx => view.focusLink(terminal, cx), t, { ghost: true, size: 11.5, px: 5, height: 22, color: t.secondary }));
    slot.child(button(`trace-${row.key}`, open ? (narrow ? 'Hide' : 'Hide trace') : 'Trace',
      cx => view.toggleReceipt(row.key, cx), t, { ghost: true, size: 11.5, px: 5, height: 22, color: t.secondary }));
  }
  top.child(slot);
  box.child(top).child(div().pl(23).min_w(0).truncate().text_size(12.5).line_height(1.38)
    .text_color(row.state === 'question' ? t.blocked : row.state === 'blocked' ? t.working : t.secondary)
    .child(row.text || ''));
  if (open) {
    const trace = v_flex().ml(23).mt(4).pt(6).pb(2).border_t(1).border_color(t.border).gap(4);
    let day = null;
    for (const line of (row.trace || [])) {
      const label = dayLabel(line.ts_ms);
      if (label !== day) { day = label; trace.child(div().pt(day ? 2 : 0).text_size(11.5).font_semibold().text_color(t.muted).child(label)); }
      const lineBody = v_flex().flex_1().min_w(0).gap(2)
        .child(div().text_color(t.secondary).child(line.text || ''));
      if (line.attachments?.length) {
        lineBody.child(div().text_size(11.5).text_color(t.muted).truncate()
          .child(`Attached: ${line.attachments.map(a => a.label).filter(Boolean).join(', ')}`));
      }
      trace.child(h_flex().gap(10).items_start().text_size(12.5).line_height(1.42)
        .child(mono(div().w(42).flex_shrink_0().pt(1).text_size(11.5).text_color(t.muted).child(clock(line.ts_ms))))
        .child(lineBody));
    }
    if (row.text && !(row.trace || []).some(line => line.text?.includes(row.text)))
      trace.child(div().pl(52).text_size(12).line_height(1.4).text_color(t.muted).child(row.text));
    const liveMap = view.store.wsArtifactMap?.(row.ws) || null;
    const evidence = workstreamAttachments(row.source || { id: row.ws, tasks: [] }, view.store.log?.entries || [], liveMap);
    if (evidence.length) {
      const stagedCount = evidence.filter(a => a.mode === 'staged' && a.availability === 'available').length;
      const summaryNote = row.wsState === 'cleaned'
        ? `worktree cleaned · ${stagedCount} staged`
        : `${stagedCount} staged`;
      const showAll = view.expandedEvidence?.has?.(row.ws);
      const shown = showAll ? evidence : evidence.slice(0, 6);
      const section = v_flex().id(`evidence-${row.ws}`).mt(6).pt(6).border_t(1).border_color(t.border).gap(4);
      section.child(h_flex().gap(8).items_center().min_w(0).pb(2)
        .child(div().text_size(11.5).font_semibold().text_color(t.secondary).child(`Evidence · ${evidence.length}`))
        .child(div().flex_1())
        .child(mono(div().flex_shrink_0().text_size(11).text_color(t.muted).child(summaryNote))));
      for (const att of shown) section.child(attachmentRow(view, att, t, `trace-${row.ws}`));
      if (evidence.length > 6) {
        section.child(button(`evidence-more-${row.ws}`, showAll ? 'Show fewer' : `Show ${evidence.length - 6} more`,
          cx => view.toggle(view.expandedEvidence, row.ws, cx), t,
          { ghost: true, size: 11.5, px: 6, height: 22, color: t.accent }).self_start());
      }
      trace.child(section);
    }
    const links = h_flex().gap(6).pt(4).flex_wrap();
    (row.links || []).forEach((link, i) => links.child(button(`receipt-link-${row.key}-${i}`,
      link.kind === 'terminal' ? 'Open terminal' : 'Show space',
      cx => view.focusLink(link, cx), t, { size: 11.5, px: 8, height: 24 })));
    trace.child(links);
    box.child(trace);
  }
  return box;
}

function entry(view, row, t, prev = null, next = null) {
  let body, pt = 0, pb = 8;
  if (row.kind === 'day' && !row.collapsible) {
    body = dayHeaderRow(row.label, t, prev ? 8 : 2);
    pb = 4;
  } else if (row.kind === 'day') {
    const label = `${row.collapsed ? '▸' : '▾'} ${row.label}${row.collapsed ? ` — ${row.countLabel}${row.summary ? `, ${row.summary}` : ''}` : ''}`;
    body = button(`day-${row.key}`, label, cx => { view.toggle(view.expandedDays, row.day, cx); view.follow = false; }, t,
      { size: 12, px: 8, height: 26, bg: t.hover, color: t.secondary }).w_full().justify_start().border(0);
    pb = 4;
  } else if (row.kind === 'receipt') {
    body = receipt(view, row, t);
    pb = next?.kind === 'receipt' ? 5 : 8;
  } else if (row.kind === 'person') {
    pt = prev && prev.kind !== 'day' ? 4 : 0;
    pb = next?.kind === 'receipt' ? 4 : 8;
    body = h_flex().gap(10).items_start().px(12).py(8).rounded(7).border(1).border_color(t.borderStrong).bg(t.card)
      .child(div().flex_1().min_w(0).text_size(13.5).line_height(1.45).text_color(t.foreground).child(row.text || ''))
      .child(mono(div().pt(2).flex_shrink_0().text_size(11.5).text_color(t.muted).child(row.sending ? 'sending…' : clock(row.ts_ms))));
  } else if (row.kind === 'reply') {
    pt = 2; pb = 10;
    body = div().px(2).min_w(0)
      .child(TextView.markdown(`body-${row.key}`, row.text || '').selectable(true).scrollable(false).text_size(13.5).line_height(1.52));
  } else if (row.kind === 'echo') {
    pb = 7;
    body = h_flex().gap(8).items_center().min_w(0).px(10).py(5).rounded(6).bg(t.hover).text_size(12.5).text_color(t.secondary)
      .child(glyph(row.state, t, view))
      .child(div().flex_1().min_w(0).truncate().child(`${row.title || 'Workstream'}${row.text ? ` — ${row.text}` : ''}`))
      .child(button(`echo-${row.key}`, '↑', cx => view.jumpReceipt(row.receiptKey, cx), t,
        { ghost: true, label: 'Show receipt', px: 6, height: 22, color: t.secondary }));
  } else {
    body = div().text_size(13.5).line_height(1.45).child(row.text || '');
  }
  const wrap = v_flex().id(`entry-${row.key}`).min_w(0).flex_shrink_0().pb(pb);
  if (pt) wrap.pt(pt);
  return wrap.on_size_change((size, cx) => {
      if (view.rowHeights.get(row.key) === size.height) return;
      view.rowHeights.set(row.key, size.height);
      // Rows that grow at the bottom (an opened trace, a long reply) stay in view.
      if (view.follow) view.latest(cx); else cx.notify();
    }).child(body);
}

const toneGlyph = (tone, t) => {
  const cell = h_flex().w(14).h(18).flex_shrink_0().items_center().justify_center();
  return tone === 'quiet' ? cell.child(div().size(4).rounded(2).bg(t.gutter))
    : cell.text_size(12.5).text_color(tone === 'done' ? t.done : tone === 'question' ? t.blocked : t.working)
      .child(({ done: '✓', question: '?', blocked: '!' })[tone]);
};

function journalEvent(view, row, t) {
  const narrow = isNarrow(view);
  const open = view.expandedJournal.has(row.key);
  const rawAtts = Array.isArray(row.source?.data?.attachments) ? row.source.data.attachments : [];
  const sentence = div().flex_1().min_w(0).text_size(12.5).line_height(1.4)
    .text_color(row.system ? t.muted : open ? t.foreground : t.secondary).child(row.text);
  if (!open) sentence.truncate();
  const line = h_flex().w_full().gap(9).items_start().min_w(0)
    .child(mono(div().w(40).flex_shrink_0().pt(1).text_size(11.5).text_color(t.muted).child(clock(row.ts_ms))))
    .child(toneGlyph(row.tone, t)).child(sentence);
  if (rawAtts.length) {
    line.child(mono(div().flex_shrink_0().pt(1).text_size(11).text_color(t.muted)
      .child(`· ${rawAtts.length} ${rawAtts.length === 1 ? 'file' : 'files'}`)));
  }
  const toggle = Button.new(`journal-${row.key}`).w_full().h_auto().px(6).py(4).rounded(5).justify_start()
    .hover(s => s.bg(t.hover)).accessibility_label(row.text)
    .on_click((_e, cx) => view.toggleJournalRow ? view.toggleJournalRow(row, cx) : view.toggle(view.expandedJournal, row.key, cx));
  if (open) toggle.bg(t.hover);
  const item = v_flex().w_full().min_w(0).flex_shrink_0().child(toggle.child(line));
  if (!open) return item;
  const fields = v_flex().ml(narrow ? 10 : 56).mr(4).mt(3).mb(8).px(10).py(7).rounded(6)
    .border(1).border_color(t.border).bg(t.recessed).gap(4).text_size(11.5).line_height(1.4);
  for (const [label, value] of journalFields(row)) fields.child(h_flex().gap(10).items_start().min_w(0)
    .child(div().w(76).flex_shrink_0().text_color(t.muted).child(label))
    .child(mono(div().flex_1().min_w(0).text_color(t.secondary).child(value))));
  const liveMap = view.store.wsArtifactMap?.(row.ws) || null;
  const atts = reportAttachments(row.source, liveMap);
  if (atts.length) {
    const attWrap = v_flex().mt(4).pt(6).border_t(1).border_color(t.border).gap(4)
      .child(div().text_size(11.5).font_semibold().text_color(t.secondary).child(`Attachments (${atts.length})`));
    for (const att of atts) attWrap.child(attachmentRow(view, att, t, `journal-${row.source.seq}`));
    fields.child(attWrap);
  }
  return item.child(fields);
}

function journal(view, t) {
  const pad = pagePad(view);
  const rows = journalRows(view.store.journal || view.store.log, view.store.board, { filter: view.filter.value(), showSystem: view.showSystem });
  const start = Math.min(view.journalStart || 0, Math.max(0, rows.length - 1));
  const body = v_flex().id('journal-list').flex_1().min_h(0).overflow_y_scrollbar().px(pad).pt(4).pb(14);
  if (start) body.child(button('journal-newer', 'Show newer', cx => { view.journalStart = Math.max(0, start - 120); cx.notify(); }, t, { size: 11.5 }).mt(6));
  const slice = rows.slice(start, start + 120);
  for (let i = 0; i < slice.length; i++) {
    const row = slice[i];
    if (row.kind === 'day') body.child(dayHeaderRow(row.label, t, i === 0 ? 4 : 12));
    else if (row.kind === 'group') body.child(div().pt(8).pb(3).px(6).text_size(13).font_semibold()
      .text_color(row.ws ? t.foreground : t.muted).truncate().child(row.title));
    else body.child(journalEvent(view, row, t));
  }
  if (!rows.length) body.child(div().p(12).text_size(13).text_color(t.muted).child(view.filter.value() ? 'No events match the filter.' : 'Nothing recorded yet.'));
  if (start + 120 < rows.length) body.child(button('journal-older-loaded', 'Show older', cx => { view.journalStart = start + 120; cx.notify(); }, t, { size: 11.5 }).mt(8));
  else if (view.store.journal?.has_more) body.child(button('journal-older', view.store.loadingOlder ? 'Loading…' : 'Load older', cx => view.store.loadOlder(cx, view), t, { size: 11.5, disabled: view.store.loadingOlder }).mt(8));
  if (view.store.journalError) body.child(div().pt(6).text_size(12).text_color(t.blocked).child(view.store.journalError));
  const indicator = h_flex().size(15).items_center().justify_center().rounded(3).border(1).border_color(view.showSystem ? t.accent : t.borderStrong)
    .bg(view.showSystem ? t.accent : t.input).flex_shrink_0();
  if (view.showSystem) indicator.child(div().size(7).rounded(1).bg(t.accent_foreground));
  return v_flex().flex_1().min_h(0).child(h_flex().gap(10).items_center().flex_wrap().px(pad).py(9).border_b(1).border_color(t.border)
    .child(h_flex().flex_1().min_w(160).h(30).px(10).items_center().rounded(6).border(1).border_color(t.borderStrong).bg(t.input).child(Input.new(view.filter).w_full()))
    .child(Checkbox.new('journal-system').checked(!!view.showSystem).accessibility_label('Show system events')
      .on_change((checked, cx) => { view.showSystem = checked; view.journalStart = 0; cx.notify(); })
      .child(h_flex().gap(7).items_center().text_size(12).text_color(t.secondary).child(indicator).child('System events'))))
    .child(body);
}

const roleSourceColor = (source, t) => source === 'override' ? t.working : source === 'file' ? t.accent : t.muted;

function roleNoticeBar(view, t) {
  const notice = view.store.rolesNoticeUntil && Date.now() > view.store.rolesNoticeUntil ? '' : view.store.rolesNotice;
  const error = view.store.rolesError;
  if (!error && !notice) return null;
  return div().px(pagePad(view)).pt(8).text_size(12).text_color(error ? t.blocked : t.done).child(error || notice);
}

function roleRow(view, role, t) {
  const narrow = isNarrow(view);
  const confirming = view.confirmRoleAction?.name === role.name ? view.confirmRoleAction.action : null;
  const active = !!confirming || view.hoveredRole === role.name;
  const removable = role.source === 'file' ? 'delete' : role.source === 'override' ? 'reset' : null;
  const row = v_flex().id(`role-row-${role.name}`).px(10).py(8).rounded(6).gap(3).flex_shrink_0()
    .border_b(1).border_color(t.border)
    .bg(active ? t.hover : t.transparent)
    .on_hover((hovered, cx) => {
      const next = hovered ? role.name : view.hoveredRole === role.name ? null : view.hoveredRole;
      if (next !== view.hoveredRole) { view.hoveredRole = next; cx.notify(); }
    });
  const top = h_flex().h(24).gap(8).items_center().min_w(0);
  top.child(Button.new(`role-open-${role.name}`).flex_1().min_w(0).h(24).justify_start()
    .accessibility_label(`View role ${role.name}`)
    .on_click((_e, cx) => { cx.stop_propagation(); view.openRoleDetail(role.name, cx); })
    .child(div().min_w(0).truncate().text_size(13.5).font_semibold().text_color(t.foreground).child(role.name)));
  // One fixed-size slot holds either the source label or hover/confirm actions, so hovering never shifts layout.
  const slot = h_flex().w(narrow ? 156 : 188).h(24).flex_shrink_0().gap(4).items_center().justify_end().overflow_hidden();
  if (confirming) {
    slot.child(button(`role-confirm-${role.name}`, confirming === 'reset' ? 'Confirm reset' : 'Confirm delete',
      cx => view.runRemoveRole(role.name, confirming, cx), t,
      { ghost: true, size: 11.5, px: 6, height: 22, color: confirming === 'reset' ? t.working : t.blocked, disabled: view.store.savingRole }));
    slot.child(button(`role-cancel-${role.name}`, 'Cancel',
      cx => { view.confirmRoleAction = null; cx.notify(); }, t, { ghost: true, size: 11.5, px: 5, height: 22, color: t.secondary }));
  } else if (active) {
    slot.child(button(`role-edit-${role.name}`, 'Edit',
      cx => view.openRoleEdit(role.name, cx), t, { ghost: true, size: 11.5, px: 6, height: 22, color: t.secondary }));
    slot.child(button(`role-copy-${role.name}`, 'Copy',
      cx => view.openRoleCreate(role.name, cx), t, { ghost: true, size: 11.5, px: 6, height: 22, color: t.secondary }));
    if (removable) slot.child(button(`role-${removable}-${role.name}`, removable === 'reset' ? 'Reset' : 'Delete',
      cx => view.confirmRemoveRole(role.name, removable, cx), t,
      { ghost: true, size: 11.5, px: 6, height: 22, color: removable === 'reset' ? t.working : t.blocked }));
  } else {
    slot.child(div().px(7).py(1).rounded(9).bg(t.hover).text_size(11.5).text_color(roleSourceColor(role.source, t)).child(role.source));
  }
  top.child(slot);
  row.child(top);
  row.child(h_flex().h(19).items_center().min_w(0).text_size(12).text_color(t.secondary)
    .child(div().min_w(0).truncate().child(roleSummaryLine(role))));
  return row;
}

function roleList(view, t) {
  const pad = pagePad(view);
  const allRoles = view.store.roles || [];
  const filtered = filterRoles(allRoles, view.roleFilter?.value?.() || '');
  const toolbar = h_flex().gap(10).items_center().px(pad).py(9).border_b(1).border_color(t.border)
    .child(h_flex().flex_1().min_w(0).h(30).px(10).items_center().rounded(6).border(1).border_color(t.borderStrong).bg(t.input)
      .child(Input.new(view.roleFilter).w_full()))
    .child(button('roles-new', '+ New role', cx => view.openRoleCreate('', cx), t, { primary: true, height: 30, px: 11 }));
  const list = v_flex().id('roles-list').flex_1().min_h(0).overflow_y_scrollbar().px(pad).pt(6).pb(14).gap(2);
  if (view.store.loadingRoles && !view.store.roles) list.child(div().p(8).text_size(13).text_color(t.muted).child('Loading roles…'));
  else if (!filtered.length) list.child(div().p(8).text_size(13).text_color(t.muted).child(
    view.roleFilter?.value?.() ? 'No roles match the filter.' : 'No roles found.'));
  else for (const role of filtered) list.child(roleRow(view, role, t));
  const root = v_flex().flex_1().min_h(0).child(toolbar);
  const banner = roleNoticeBar(view, t);
  if (banner) root.child(banner);
  return root.child(list);
}

function roleDetail(view, t) {
  const pad = pagePad(view);
  const role = view.roleByName(view.selectedRole);
  if (!role) return roleList(view, t);
  const removable = role.source === 'file' ? 'delete' : role.source === 'override' ? 'reset' : null;
  const confirming = view.confirmRoleAction?.name === role.name ? view.confirmRoleAction.action : null;
  const topbar = h_flex().gap(8).items_center().flex_wrap().px(pad).py(9).border_b(1).border_color(t.border)
    .child(div().flex_1().min_w(140).child(h_flex().gap(8).items_center().min_w(0)
      .child(div().text_size(13.5).font_semibold().truncate().text_color(t.foreground).child(role.name))
      .child(div().px(7).py(1).rounded(9).bg(t.hover).text_size(11.5).text_color(roleSourceColor(role.source, t)).child(role.source))));
  if (confirming) {
    topbar.child(button('role-detail-confirm', confirming === 'reset' ? 'Confirm reset' : 'Confirm delete',
      cx => view.runRemoveRole(role.name, confirming, cx), t,
      { height: 28, color: confirming === 'reset' ? t.working : t.blocked, disabled: view.store.savingRole }));
    topbar.child(button('role-detail-cancel-remove', 'Cancel',
      cx => { view.confirmRoleAction = null; cx.notify(); }, t, { height: 28 }));
  } else {
    topbar.child(button('role-detail-edit', 'Edit', cx => view.openRoleEdit(role.name, cx), t, { primary: true, height: 28 }));
    topbar.child(button('role-detail-copy', 'Copy', cx => view.openRoleCreate(role.name, cx), t, { height: 28 }));
    if (removable) topbar.child(button(`role-detail-${removable}`, removable === 'reset' ? 'Reset' : 'Delete',
      cx => view.confirmRemoveRole(role.name, removable, cx), t,
      { height: 28, color: removable === 'reset' ? t.working : t.blocked }));
  }
  const fields = [
    ...(role.description ? [['About', role.description, false, t.foreground]] : []),
    ...(role.available === false ? [['Status', role.unavailable_reason || 'Cannot start on this machine', false, t.working]] : []),
    ['Harness', role.harness || '—', false, t.foreground],
    ['Model', role.model || '—', true, t.foreground],
    ['Effort', role.effort || '—', false, t.foreground],
    ['Session', `${role.session || 'none'}${role.resumable ? ' · resumable' : ''}${role.steering ? ' · steering' : ''}`, false, t.foreground],
    ['Start', role.start || '—', true, t.secondary],
    ['Resume', role.resume || '—', true, t.secondary],
    ...(role.trust ? [['Trust', role.trust, true, t.secondary]] : []),
    ...(role.fallback?.length ? [['Fallback', role.fallback.join(' → '), false, t.secondary]] : []),
  ];
  const body = v_flex().id('roles-detail').flex_1().min_h(0).overflow_y_scrollbar().px(pad).pt(12).pb(14).gap(12);
  const meta = v_flex().gap(7);
  for (const [label, value, isMono, color] of fields) {
    const val = div().flex_1().min_w(0).text_size(isMono ? 12 : 12.5).line_height(1.42).text_color(color).child(value);
    meta.child(h_flex().gap(10).items_start().min_w(0)
      .child(div().w(74).flex_shrink_0().text_size(12).text_color(t.muted).child(label))
      .child(isMono ? mono(val) : val));
  }
  body.child(meta);
  body.child(div().h(1).bg(t.border));
  body.child(v_flex().gap(6)
    .child(div().text_size(12).font_semibold().text_color(t.muted).child('Prompt'))
    .child(div().px(12).py(10).rounded(6).border(1).border_color(t.borderStrong).bg(t.recessed).text_size(13).line_height('20px')
      .text_color(role.body ? t.foreground : t.muted)
      .child(role.body || '(empty prompt)')));
  const root = v_flex().flex_1().min_h(0).child(topbar);
  const banner = roleNoticeBar(view, t);
  if (banner) root.child(banner);
  return root.child(body);
}

function roleFieldRow(label, control, t, hint = '') {
  const wrap = v_flex().gap(5).min_w(0);
  const head = h_flex().gap(8).items_center().flex_wrap()
    .child(div().text_size(12).font_semibold().text_color(t.secondary).child(label));
  if (hint) head.child(div().text_size(11.5).text_color(t.muted).child(hint));
  return wrap.child(head).child(control);
}

function roleInputBox(state, t, isMono = false) {
  const input = Input.new(state).w_full();
  return h_flex().h(30).px(10).items_center().rounded(6).border(1).border_color(t.borderStrong).bg(t.input)
    .child(isMono ? mono(input) : input);
}

function roleForm(view, t) {
  const narrow = isNarrow(view), pad = pagePad(view);
  const isCreate = view.rolesMode === 'create';
  const existing = !isCreate ? view.roleByName(view.selectedRole) : null;
  const saving = !!view.store.savingRole;
  const nameValue = isCreate ? (view.roleFields.name.value?.() || '').trim() : (view.selectedRole || '');
  const titleText = isCreate ? (view.roleFrom ? `New role · from ${view.roleFrom}` : 'New role') : `Edit ${view.selectedRole}`;
  const subtitle = !isCreate && existing?.source === 'builtin' ? 'Saves as override' : '';
  const titleRow = h_flex().gap(8).items_center().min_w(0)
    .child(div().text_size(13.5).font_semibold().truncate().text_color(t.foreground).child(titleText));
  if (subtitle) titleRow.child(div().text_size(11.5).text_color(t.working).child(subtitle));
  const topbar = h_flex().gap(8).items_center().px(pad).py(9).border_b(1).border_color(t.border)
    .child(div().flex_1().min_w(0).child(titleRow))
    .child(button('role-form-cancel', 'Cancel', cx => view.rolesBack(cx), t, { height: 28, disabled: saving }))
    .child(button('role-form-save', saving ? 'Saving…' : isCreate ? 'Create' : 'Save',
      cx => view.saveRole(cx), t, { primary: true, height: 28, disabled: saving || !nameValue }));
  const form = v_flex().id('roles-form').flex_1().min_h(0).overflow_y_scrollbar().px(pad).pt(12).pb(14).gap(12);
  if (isCreate) {
    const templateMenu = v_flex().id('role-from-menu').role('menu').w(228).max_h(260).overflow_y_scrollbar()
      .p(5).gap(2).rounded(7).border(1).border_color(t.borderStrong).bg(t.card).shadow_lg();
    for (const r of (view.store.roles || [])) {
      templateMenu.child(Button.new(`role-from-item-${r.name}`).role('menu_item').w_full().h(28).px(8).rounded(5).justify_start()
        .text_size(12.5).text_color(t.foreground).hover(s => s.bg(t.hover)).accessibility_label(`Copy from ${r.name}`)
        .on_click((_e, cx) => { cx.stop_propagation(); view.applyRoleTemplate(r.name, cx); })
        .child(h_flex().w_full().gap(6).items_center().min_w(0)
          .child(div().flex_1().min_w(0).truncate().child(r.name))
          .child(div().text_size(11.5).text_color(t.muted).child(r.harness))));
    }
    const startFrom = h_flex().gap(6).items_center().flex_wrap()
      .child(button('role-from-blank', 'Blank', cx => view.applyRoleTemplate('', cx), t,
        { height: 26, size: 12, primary: !view.roleFrom }))
      .child(Popover.new('role-from-popover').anchor('top_left').open(!!view.fromMenuOpen)
        .on_open_change((open, cx) => { view.fromMenuOpen = open; cx.notify(); })
        .trigger(Button.new('role-from-trigger').h(26).px(10).rounded(5).border(1)
          .border_color(view.roleFrom ? t.accent : t.borderStrong).bg(view.roleFrom ? t.hover : t.card)
          .text_size(12).text_color(t.foreground).hover(s => s.bg(t.hover))
          .accessibility_label('Copy from existing role')
          .child(view.roleFrom ? `From: ${view.roleFrom} ▾` : 'Copy from… ▾'))
        .content(templateMenu));
    form.child(roleFieldRow('Start from', startFrom, t));
    form.child(roleFieldRow('Name', roleInputBox(view.roleFields.name, t, true), t, 'letters, digits, - or _'));
  }
  const currentHarness = (view.roleFields.harness.value?.() || '').trim();
  const harnessChips = h_flex().gap(5).flex_wrap();
  for (const h of ROLE_HARNESSES) {
    harnessChips.child(button(`role-harness-${h}`, h, cx => view.pickRoleHarness(h, cx), t,
      { height: 24, px: 8, size: 11.5, primary: currentHarness === h }));
  }
  form.child(roleFieldRow('Harness', v_flex().gap(5).child(roleInputBox(view.roleFields.harness, t, true)).child(harnessChips), t));
  const modelField = roleFieldRow('Model', roleInputBox(view.roleFields.model, t, true), t);
  const effortField = roleFieldRow('Effort', roleInputBox(view.roleFields.effort, t, true), t);
  if (narrow) form.child(modelField).child(effortField);
  else form.child(h_flex().gap(10).min_w(0)
    .child(div().flex_1().min_w(0).child(modelField))
    .child(div().w(180).flex_shrink_0().child(effortField)));
  const currentSession = (view.roleFields.session.value?.() || '').trim();
  const sessionChips = h_flex().gap(5).flex_wrap();
  for (const s of ROLE_SESSIONS) {
    sessionChips.child(button(`role-session-${s}`, s, cx => { view.roleFields.session.set_value(s); cx.notify(); }, t,
      { height: 24, px: 8, size: 11.5, primary: currentSession === s }));
  }
  form.child(roleFieldRow('Session', v_flex().gap(5).child(roleInputBox(view.roleFields.session, t, true)).child(sessionChips), t));
  form.child(roleFieldRow('Start command', roleInputBox(view.roleFields.start, t, true), t, 'must include {brief}'));
  form.child(roleFieldRow('Resume command', roleInputBox(view.roleFields.resume, t, true), t, 'optional'));
  form.child(roleFieldRow('Trust flags', roleInputBox(view.roleFields.trust, t, true), t, 'optional'));
  form.child(roleFieldRow('Prompt', div().px(10).py(8).rounded(6).border(1).border_color(t.borderStrong).bg(t.input)
    .child(Textarea.new(view.roleFields.body).w_full().text_size(13)), t));
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

function artifactViewer(view, t) {
  const pad = pagePad(view);
  const v = view.store.artifactViewer;
  if (!v?.attachment) return div().p(pad).text_size(13).text_color(t.muted).child('No attachment selected.');
  const att = v.attachment;
  const act = attachmentActions(att);
  const confirmKey = `viewer:${att.id}`;
  const confirmHtml = view.confirmHtmlId === confirmKey;
  const prov = [att.task, att.report_kind ? `Reported ${att.report_kind}` : '', att.ts_ms ? clock(att.ts_ms) : '',
    formatBytes(v.total_bytes || att.bytes), att.mode].filter(Boolean).join(' · ');
  const topbar = v_flex().px(pad).py(9).gap(6).border_b(1).border_color(t.border);
  const topRow = h_flex().gap(8).items_center().flex_wrap()
    .child(v_flex().flex_1().min_w(160).gap(2)
      .child(h_flex().gap(7).items_center().min_w(0)
        .child(kindPill(att.kind, t))
        .child(div().flex_1().min_w(0).truncate().text_size(13).font_semibold().text_color(t.foreground).child(att.task_title || att.label)))
      .child(mono(div().truncate().text_size(11).text_color(t.muted).child(prov))));
  if (act.canOpen) topRow.child(button('artifact-viewer-open', 'Open',
    cx => view.store.openAttachment(cx, view, att), t, { height: 26, size: 12 }));
  if (act.canBrowser) topRow.child(button('artifact-viewer-browser', 'Browser…',
    cx => { view.confirmHtmlId = confirmHtml ? null : confirmKey; cx.notify(); }, t, { height: 26, size: 12, color: t.working }));
  if (act.canReveal) topRow.child(button('artifact-viewer-reveal', 'Reveal',
    cx => view.store.revealAttachment(cx, view, att), t, { height: 26, size: 12 }));
  topRow.child(button('artifact-viewer-copy', 'Copy path',
    cx => view.store.copyAttachmentPath(cx, view, att), t, { height: 26, size: 12 }));
  topbar.child(topRow);
  if (confirmHtml) {
    topbar.child(h_flex().gap(8).items_center().flex_wrap().px(8).py(6).rounded(6).border(1).border_color(tint(t.working, '70')).bg(tint(t.working, '10'))
      .child(div().flex_1().min_w(140).text_size(11.5).text_color(t.working).child('Opens local HTML in your browser; scripts can run.'))
      .child(button('artifact-viewer-html-confirm', 'Open in browser',
        cx => view.store.openAttachment(cx, view, att, { allowHtml: true }), t, { height: 24, size: 11.5, color: t.working }))
      .child(button('artifact-viewer-html-cancel', 'Cancel',
        cx => { view.confirmHtmlId = null; cx.notify(); }, t, { ghost: true, height: 24, size: 11.5, color: t.secondary })));
  }
  const body = v_flex().id('artifact-viewer-body').flex_1().min_h(0).overflow_y_scrollbar().px(pad).py(10).gap(8);
  const bannerColor = v.truncated ? t.working : t.muted;
  const bannerText = v.truncated
    ? `Showing first ${formatBytes(v.shown_bytes)} of ${formatBytes(v.total_bytes)} — Reveal or Open for full file`
    : 'Read-only plain text (ANSI & control characters stripped)';
  body.child(h_flex().gap(8).items_center().px(10).py(6).rounded(6).border(1).border_color(t.border).bg(t.recessed)
    .child(div().flex_1().min_w(0).text_size(11.5).text_color(bannerColor).child(bannerText))
    .child(mono(div().flex_shrink_0().text_size(11).text_color(t.muted).child(`${formatBytes(v.shown_bytes || 0)} / 64 KB cap`))));
  if (v.loading) {
    body.child(div().p(10).text_size(12.5).text_color(t.muted).child('Reading plain text…'));
  } else if (v.error) {
    body.child(div().p(10).text_size(12.5).text_color(t.blocked).child(v.error));
  } else {
    const lines = String(v.text || '').split('\n');
    const shownLines = lines.slice(0, 1200);
    const code = v_flex().px(10).py(8).rounded(6).border(1).border_color(t.borderStrong).bg(t.recessed).gap(1);
    shownLines.forEach((line, i) => {
      code.child(h_flex().gap(10).items_start().min_w(0)
        .child(mono(div().w(30).flex_shrink_0().text_right().text_size(11.5).line_height(1.5).text_color(t.muted).child(String(i + 1))))
        .child(mono(div().flex_1().min_w(0).text_size(12).line_height(1.5).text_color(t.foreground).child(line || ' '))));
    });
    if (lines.length > 1200) {
      code.child(div().pt(6).text_size(11.5).text_color(t.muted).child(`… ${lines.length - 1200} more lines omitted`));
    }
    body.child(code);
  }
  const notice = view.store.noticeUntil && Date.now() > view.store.noticeUntil ? '' : view.store.notice;
  const root = v_flex().flex_1().min_h(0).child(topbar).child(body);
  if (notice) root.child(div().px(pad).pb(8).text_size(12).text_color(t.muted).child(notice));
  return root;
}

export function renderRail(view) {
  const t = palette(), vm = view.model(), pad = pagePad(view), isJournal = view.page === 'journal', isRoles = view.page === 'roles', isArtifact = view.page === 'artifact';
  const state = vm.working ? `Working · ${vm.elapsed}` : vm.status === 'off' ? 'Off' : vm.available ? 'Idle' : 'Degraded';
  const stateColor = vm.working ? t.working : vm.status === 'off' ? t.muted : vm.available ? t.done : t.blocked;
  const header = h_flex().h(40).flex_shrink_0().px(pad).gap(8).items_center().border_b(1).border_color(t.border);
  if (isJournal) header.child(button('journal-back', '←', cx => { view.page = 'stream'; view.follow = true; view.latest(cx); }, t, { ghost: true, px: 5, height: 26, size: 13.5, color: t.secondary, label: 'Back to stream' }));
  if (isRoles) header.child(button('roles-back', '←', cx => view.rolesBack(cx), t, { ghost: true, px: 5, height: 26, size: 13.5, color: t.secondary, label: view.rolesMode === 'list' ? 'Back to stream' : 'Back to roles' }));
  if (isArtifact) header.child(button('artifact-back', '←', cx => view.closeArtifactViewer ? view.closeArtifactViewer(cx) : (view.page = 'stream', cx.notify()), t, { ghost: true, px: 5, height: 26, size: 13.5, color: t.secondary, label: 'Back' }));
  const headerTitle = isJournal ? 'Journal' : isRoles ? 'Roles' : isArtifact ? (view.store.artifactViewer?.attachment?.label || 'Attachment') : 'Coordinator';
  header.child(div().text_size(14).font_semibold().truncate().text_color(t.foreground).child(headerTitle));
  if (!isJournal && !isRoles && !isArtifact) {
    header.child(h_flex().gap(6).items_center().px(8).h(22).rounded(11)
      .bg(vm.working ? tint(t.working, '20') : t.hover)
      .child(div().size(6).rounded(3).bg(stateColor))
      .child(div().text_size(12).text_color(vm.working ? t.working : vm.status === 'off' ? t.muted : vm.available ? t.secondary : t.blocked).child(state)));
  }
  header.child(div().flex_1()).child(Popover.new('rail-actions').anchor('top_right').open(view.menuOpen)
    .on_open_change((open, cx) => { view.menuOpen = open; if (!open) view.confirmRestart = false; cx.notify(); })
    .trigger(Button.new('rail-more').h(26).w(28).rounded(5).border(1)
      .border_color(view.menuFocus?.is_focused?.() ? t.accent : t.transparent)
      .text_color(t.secondary).hover(s => s.bg(t.hover).text_color(t.foreground))
      .track_focus(view.menuFocus).accessibility_label('Coordinator menu').child('⋯'))
    .content(menu(view, vm, t)))
    .child(button('rail-close', '×', () => view.dismiss(), t, { ghost: true, px: 7, height: 26, size: 14, color: t.secondary, label: 'Close coordinator' }));
  const root = v_flex().id('rail-dialog').relative().size_full().min_h(0).min_w(0).bg(t.popover).text_color(t.foreground).text_size(13.5)
    .role('dialog').accessibility_label('Coordinator').track_focus(view.rootFocus).on_key_down((event, cx) => view.key(event, cx)).child(header);
  if (isJournal) return root.child(journal(view, t));
  if (isRoles) return root.child(rolesView(view, t));
  if (isArtifact) return root.child(artifactViewer(view, t));
  if (vm.needsYou?.length) root.child(div().px(pad).pt(8).pb(2).child(needs(view, vm, t)));
  const body = v_flex().id('rail-log').flex_1().min_h(0).min_w(0).overflow_y_scrollbar().track_scroll(view.scroll)
    .track_focus(view.logFocus).tab_index(1).role('log').accessibility_label('Coordinator stream').px(pad).pt(10)
    .on_size_change((size, cx) => { if (view.logHeight !== size.height) { view.logHeight = size.height; if (view.follow) view.latest(cx); else cx.notify(); } })
    .on_scroll_wheel((event, cx) => view.scrolled(event.delta.y, cx));
  let children = 0;
  const add = element => { body.child(element); children++; };
  if (view.windowStart > 0) add(button('rail-older', 'Show older entries', cx => view.older(cx), t, { size: 11.5, height: 24 }).mb(6));
  if (!vm.rows.length) add(div().py(8).text_size(13).text_color(t.muted).child('Nothing in flight. Ask for something below.'));
  const visible = vm.rows.slice(view.windowStart, view.windowStart + 100);
  visible.forEach((row, i) => add(entry(view, row, t, visible[i - 1] || null, visible[i + 1] || null)));
  if (view.store.pending && view.follow) add(entry(view, view.store.pending, t, visible.at(-1) || null, null));
  // Revealing this last child is how the stream scrolls to the bottom.
  add(div().id('rail-end').h(1).flex_shrink_0());
  if (view.pinBottom) { view.pinBottom = false; view.scroll.scroll_to_item(children - 1); }
  root.child(body);
  if (!view.follow || view.windowStart + 100 < vm.rows.length) root.child(h_flex().justify_center().py(4)
    .child(button('rail-latest', 'Jump to latest ↓', cx => view.latest(cx), t, { size: 11.5, height: 26, pill: true, color: t.secondary })));
  if (vm.working) {
    const dot = div().size(6).rounded(3).bg(t.working);
    if (!view.reducedMotion) dot.loop('opacity', { from: .3, to: 1, period: 1200 });
    root.child(h_flex().px(pad).gap(8).h(26).items_center().text_size(12).text_color(t.secondary)
      .child(dot).child(div().flex_1().min_w(0).truncate().child(vm.step))
      .child(mono(div().flex_shrink_0().text_size(11.5).text_color(t.muted).child(vm.elapsed))));
  }
  const composer = v_flex().flex_shrink_0().gap(6).px(pad).pt(4).pb(12);
  if (view.question) composer.child(h_flex().gap(8).items_center().min_w(0).px(10).py(4).rounded(6).border(1).border_color(t.border).bg(t.card)
    .text_size(12).text_color(t.secondary)
    .child(div().min_w(0).flex_1().truncate().child(`Answering: ${view.question.text || view.question.time}`))
    .child(button('rail-context-clear', '×', cx => { view.question = null; view.input.focus(); cx.notify(); }, t,
      { ghost: true, px: 5, height: 22, color: t.secondary, label: 'Clear reply context' })));
  if (vm.status === 'off') composer.child(h_flex().h(38).px(12).items_center().rounded(7).border(1).border_dashed().border_color(t.borderStrong).bg(t.recessed)
    .child(div().flex_1().text_size(13).text_color(t.secondary).child('Coordinator is off'))
    .child(button('rail-start', 'Start', cx => view.coordinatorAction('start-coordinator', cx), t, { color: t.accent, height: 26 })));
  else {
    const enabled = vm.canSend && !view.store.sending && !!view.draft.trim();
    const focused = !!(view.inputFocused || view.draft);
    const line = h_flex().min_h(38).gap(8).items_end().pl(12).pr(5).py(4).min_w(0).rounded(7).border(1)
      .border_color(focused ? t.accent : t.borderStrong).bg(t.input)
      .child(mono(div().flex_shrink_0().h(28).font_semibold().text_size(15).text_color(vm.working ? t.working : t.accent).child('›')));
    line.child(h_flex().flex_1().min_w(0).items_center().tab_index(4).child(Textarea.new(view.input).w_full().text_size(13.5)));
    line.child(Button.new('rail-send').tab_index(5).size(28).rounded(5).flex_shrink_0()
      .disabled(!enabled).text_color(enabled ? t.accent : t.muted).hover(s => s.bg(t.hover))
      .accessibility_label('Send message').tooltip('Send message')
      .on_click((_e, cx) => { cx.stop_propagation(); view.store.send(cx, view); })
      .child(svg(`mods/${MOD_NAME}/send.svg`).size(16).text_color(enabled ? t.accent : t.muted)));
    composer.child(line);
  }
  const notice = view.store.noticeUntil && Date.now() > view.store.noticeUntil ? '' : view.store.notice;
  if (notice || view.store.error) composer.child(div().text_size(12).text_color(view.store.error ? t.blocked : t.muted).child(view.store.error || notice));
  return root.child(composer);
}
