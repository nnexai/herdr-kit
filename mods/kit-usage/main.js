import { View, div } from 'gpui-kit';
import { h_flex, v_flex, Button, HoverCard, Progress, ProgressTrack, ProgressIndicator, ScrollHandle } from 'gpui-base';
import { agents, theme, settings, props, shared, setShared, exec, openOverlay, closeOverlay, nextCommand } from 'herdr';
import { BINARY, DATA_DIR, MOD_NAME } from './runtime.js';
import { decodeSnapshot, views, usedFraction, value, label, credits, pace, staleLimit, span, reset, errorText, providerStatusText, limitDescription, hoverSummary, formatTime, formatDateTime } from './model.js';

const KEY = 'herdr-kit.usage.snapshot.v1';
const DIALOG_WIDTH = 600;
const DIALOG_HEIGHT = 590;
let strips = 0, inflight = false, validated = false, dialog = null;

const args = () => DATA_DIR ? ['--data-dir', DATA_DIR] : [];
const working = () => agents().some(a => a.status === 'working');
const mono = el => el.font_family('JetBrains Mono');

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

function palette() {
  const t = theme();
  const light = settings().dark === false;
  const fg = t.foreground || (light ? '#1b1b1e' : '#eeeeef');
  const popover = t.popover || (light ? '#ffffff' : '#25272b');
  const rawMuted = t.muted || (light ? '#69696b' : '#98999b');
  const secondary = mix(fg, rawMuted, light ? 0.4 : 0.42);
  const muted = mix(rawMuted, fg, 0.14);
  const borderStrong = tint(fg, light ? '26' : '22');
  const track = tint(fg, light ? '14' : '16');
  const card = light
    ? mix(t.raised || '#f6f6f6', popover, 0.2)
    : mix(t.raised || '#2c2e31', fg, 0.03);
  const recessed = light
    ? mix(t.raised || '#f6f6f6', popover, 0.35)
    : mix(popover, t.surface || t.canvas || '#131518', 0.42);
  return light
    ? { ...t, foreground: fg, popover, secondary, muted, card, recessed, borderStrong, track }
    : { ...t, foreground: fg, popover, secondary, muted, card, recessed, borderStrong, track,
        working: '#e5be57', blocked: '#f47b78' };
}

function readShared() {
  try {
    const s = shared(KEY);
    return s ? decodeSnapshot(JSON.stringify(s)) : null;
  } catch (_) {
    return null;
  }
}

function meterColor(used, t) {
  return used > 0.95 ? t.blocked : used > 0.8 ? t.working : t.accent;
}

function fillPercent(used) {
  if (used <= 0) return 0;
  return Math.min(100, Math.max(1.5, Math.round(used * 10000) / 100));
}

function fluidMeter(id, used, t, height = 5) {
  const pct = Math.round(used * 10000) / 100;
  const visualPct = fillPercent(used);
  const radius = Math.ceil(height / 2);
  const track = ProgressTrack.new().w_full().h(height).rounded(radius).bg(t.track).overflow_hidden();
  if (visualPct > 0) {
    track.child(ProgressIndicator.new().w(`${visualPct}%`).h_full().rounded(radius).bg(meterColor(used, t)));
  }
  return Progress.new(id).value(pct).w_full().child(track);
}

function hoverCardContent(vm, t) {
  const card = v_flex().w(260).p(10).gap(6).rounded(7).border(1).border_color(t.borderStrong).bg(t.popover).shadow_lg();
  const stamp = vm.updated ? (vm.stale ? `Stale · ${vm.updated}` : vm.updated) : (vm.collecting ? 'Checking…' : '');
  const header = h_flex().gap(8).items_center().justify_between()
    .child(div().text_size(12).font_semibold().text_color(t.foreground).child('Subscription limits'));
  if (stamp) {
    header.child(mono(div().flex_shrink_0().text_size(11).text_color(vm.stale ? t.working : t.muted).child(stamp)));
  }
  card.child(header);
  if (vm.state === 'ready' && vm.provider && vm.window) {
    const valColor = vm.used > 0.95 ? t.blocked : vm.used > 0.8 ? t.working : vm.used === 0 ? t.muted : t.foreground;
    const peak = v_flex().gap(4)
      .child(h_flex().gap(6).items_baseline().min_w(0)
        .child(div().flex_shrink_0().text_size(12).font_medium().text_color(t.foreground).child(vm.provider))
        .child(div().flex_1().min_w(0).truncate().text_size(11.5).text_color(t.secondary).child(vm.window))
        .child(mono(div().flex_shrink_0().text_size(12).font_semibold().text_color(valColor).child(vm.amount))));
    if (vm.used !== null && !vm.unlimited) {
      const visualPct = fillPercent(vm.used);
      const track = div().w_full().h(4).rounded(2).bg(t.track).overflow_hidden();
      if (visualPct > 0) track.child(div().w(`${visualPct}%`).h_full().rounded(2).bg(meterColor(vm.used, t)));
      peak.child(track);
    }
    if (vm.credits) {
      peak.child(div().text_size(11).text_color(t.muted).child(vm.credits));
    }
    card.child(peak);
  } else {
    card.child(div().text_size(11.5).text_color(t.muted).child(vm.status || 'Reading local usage…'));
  }
  card.child(div().pt(6).border_t(1).border_color(t.border).text_size(11).text_color(t.muted).child(vm.hint));
  return card;
}

export default class Usage extends View {
  init(_p, cx) {
    const p = props() ?? {};
    this.details = p.view === 'details' || (!p.view && !!p.overlay);
    this.source = p.view === 'source';
    this.handle = cx.focus_handle();
    this.width = this.details ? DIALOG_WIDTH : 1000;
    this.lastRead = 0;
    this.collectingSince = 0;
    this.lastWorking = working();
    this.failed = false;
    this.closed = false;
    if (this.details) {
      this.scroll = ScrollHandle.new();
      this.scrollFocus = cx.focus_handle();
      this.scrollIndex = 0;
      if (p?.overlay) dialog = p.overlay;
      this.handle.focus();
    } else {
      strips++;
    }
    if (!this.details || strips === 0) this.read(cx);
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
    if (!this.details) {
      cx.spawn(async cx => {
        while (!this.closed) {
          const cmd = await nextCommand();
          if (this.closed) break;
          if (cmd === 'show') this.open();
          else if (cmd === 'focus') this.handle.focus();
        }
      });
    }
  }

  close() {
    this.closed = true;
    if (this.details) {
      if (dialog === props()?.overlay) dialog = null;
    } else {
      strips = Math.max(0, strips - 1);
    }
  }

  open() {
    if (!dialog) {
      dialog = openOverlay({
        mod: MOD_NAME,
        props: { view: 'details' },
        kind: 'dialog',
        width: DIALOG_WIDTH,
        height: DIALOG_HEIGHT,
      });
    }
  }

  read(cx) {
    if (inflight) return;
    this.lastRead = Date.now();
    inflight = true;
    cx.spawn(async cx => {
      try {
        const result = await exec(BINARY, [...args(), 'snapshot', '--json', ...(working() ? ['--agents-working'] : [])], { timeout: 45000 });
        if (result.code !== 0) throw new Error('unavailable');
        const snapshot = decodeSnapshot(result.stdout);
        validated = true;
        this.failed = false;
        setShared(KEY, snapshot);
      } catch (_) {
        this.failed = true;
        validated = false;
      } finally {
        inflight = false;
        cx.notify();
      }
    });
  }

  statusbar(snapshot, offline, t, now) {
    const all = views(snapshot, now, offline);
    const vm = hoverSummary(all, snapshot, { offline, failed: this.failed });
    const narrow = this.width < 500;
    const readout = {
      schema_version: 1,
      mod: MOD_NAME,
      label: vm.accessibilityLabel,
      tooltip: vm.tooltip,
      state: vm.state,
      provider: vm.provider,
      amount: vm.amount,
      additional: vm.additional,
      role: vm.role,
      hover: {
        state: vm.state,
        provider: vm.provider,
        window: vm.window,
        amount: vm.amount,
        used: vm.used,
        unlimited: vm.unlimited,
        credits: vm.credits,
        role: vm.role,
        stale: vm.stale,
        collecting: vm.collecting,
        providerCount: vm.providerCount,
        updated: vm.updated,
        status: vm.status,
        hint: vm.hint,
      },
    };
    const encoded = JSON.stringify(readout);
    if (encoded !== this.published) {
      this.published = encoded;
      setShared('herdr-kit.usage.readout.v1', readout);
    }
    if (this.source) return div().w(0).h(0);
    const button = Button.new('usage-readout').track_focus(this.handle).tab_index(0)
      .w(Math.min(narrow ? 120 : 220, Math.max(0, this.width - 16))).flex_shrink_0().h(26).px(6).items_center().justify_end().overflow_hidden()
      .bg(t.chrome).rounded(3).border(1)
      .border_color(this.handle.is_focused() ? t.accent : t.chrome)
      .hover(s => s.bg(t.hover).text_color(t.foreground))
      .text_size(11.8).text_color(t.dim)
      .accessibility_label(vm.accessibilityLabel).on_click(() => this.open());
    if (!snapshot) {
      button.child(this.failed ? 'Limits unavailable' : 'Limits …');
    } else if (!vm.providerCount) {
      button.child('Limits');
    } else {
      const amountColor = vm.role === 'blocked' ? t.blocked : vm.role === 'working' ? t.working : t.dim;
      button.child(h_flex().gap(5).items_center()
        .child('Limits')
        .when(!narrow && Boolean(vm.provider), el => el.child(div().text_color(t.muted).child(vm.provider)))
        .child(div().text_color(amountColor).child(vm.amount))
        .when(vm.providerCount > 1 && this.width >= 120, el => el.child(`+${vm.additional}`)));
    }
    const trigger = HoverCard.new('usage-hover').anchor('bottom_right').open_delay(350).close_delay(120)
      .trigger(button).content(hoverCardContent(vm, t));
    return h_flex().size_full().items_center().justify_end().bg(t.chrome)
      .border_t(1).border_color(t.border).text_size(11.8).text_color(t.muted).child(trigger);
  }

  detailValue(l, used, t) {
    if (l.unlimited) return div().text_size(12).text_color(t.muted).child('Unlimited');
    if (used === null) return div().text_size(12).text_color(t.muted).child('Unknown');
    const valColor = used > 0.95 ? t.blocked : used > 0.8 ? t.working : used === 0 ? t.muted : t.foreground;
    return h_flex().gap(3).items_baseline().justify_end()
      .child(mono(div().text_size(12.5).font_semibold().text_color(valColor).child(value(l))))
      .child(div().text_size(10.5).text_color(t.muted).child('used'));
  }

  detailMeta(l, account, p, offline, t, now) {
    const stale = offline || p.stale || p.error !== null || staleLimit(l, account.fetched_at_ms, now);
    const estimate = pace(l, now, stale);
    const r = reset(l.resets_at_ms, now);
    const c = credits(l, true);
    const lines = [];
    if (c) lines.push({ text: c, color: t.secondary });
    if (estimate?.runsOutMs) lines.push({ text: `Estimated to run out in ${span(estimate.runsOutMs)}`, color: t.working });
    if (r) {
      const passed = l.resets_at_ms !== null && l.resets_at_ms <= now;
      lines.push({ text: r, color: passed ? t.working : t.muted });
    }
    if (stale && !staleLimit(l, account.fetched_at_ms, now)) {
      lines.push({ text: 'Stale · awaiting a newer report', color: t.working });
    }
    return lines;
  }

  detailLimit(l, account, p, providerName, accountIndex, offline, t, now, narrow) {
    const used = usedFraction(l);
    const desc = limitDescription(l, account, p, now, offline, providerName);
    const meta = this.detailMeta(l, account, p, offline, t, now);
    const meterId = `usage-meter-${l.id}-${accountIndex}`;
    const labelColor = used === 0 ? t.muted : t.foreground;

    if (narrow) {
      const row = v_flex().gap(4).min_w(0).role('group').accessibility_label(desc)
        .child(h_flex().gap(8).items_baseline().min_w(0)
          .child(div().flex_1().min_w(0).truncate().text_size(12.5).text_color(labelColor).child(label(l)))
          .child(div().flex_shrink_0().child(this.detailValue(l, used, t))));
      if (used !== null && !l.unlimited) row.child(fluidMeter(meterId, used, t, 5));
      for (const item of meta) {
        row.child(div().min_w(0).text_size(11.5).line_height(1.3).text_color(item.color).child(item.text));
      }
      return row;
    }

    const middle = v_flex().flex_1().min_w(0).gap(3).pt(used !== null && !l.unlimited ? 6 : 1);
    if (used !== null && !l.unlimited) middle.child(fluidMeter(meterId, used, t, 5));
    for (const item of meta) {
      middle.child(div().min_w(0).text_size(11.5).line_height(1.3).text_color(item.color).child(item.text));
    }

    return h_flex().gap(12).items_start().min_w(0).role('group').accessibility_label(desc)
      .child(div().w(148).flex_shrink_0().pt(1).text_size(12.5).text_color(labelColor).child(label(l)))
      .child(middle)
      .child(div().w(88).flex_shrink_0().child(this.detailValue(l, used, t)));
  }

  detail(snapshot, offline, t, now) {
    const all = views(snapshot, now, offline);
    const narrow = this.width < 480;
    const pad = narrow ? 12 : 16;
    const updated = snapshot ? formatTime(snapshot.generated_at_ms) : null;
    const maxScroll = Math.max(0, all.length - 1);

    const headerMeta = h_flex().gap(6).items_center().flex_wrap().text_size(11.5).text_color(t.muted)
      .child('Local usage');
    if (updated) headerMeta.child(div().child(`· updated ${updated}`));
    if (snapshot?.collecting) headerMeta.child(div().text_color(t.accent).child('· Checking…'));
    if (offline && snapshot) headerMeta.child(div().text_color(t.working).child('· Stale'));

    const closeBtn = Button.new('usage-close').track_focus(this.handle).tab_index(0)
      .h(26).px(10).rounded(5).border(1)
      .border_color(this.handle.is_focused() ? t.accent : t.borderStrong)
      .bg(t.card).text_size(12).text_color(t.foreground)
      .hover(s => s.bg(t.hover))
      .accessibility_label('Close subscription limits')
      .on_click(() => closeOverlay(props().overlay))
      .child('Close');

    const header = h_flex().flex_shrink_0().px(pad).pt(13).pb(10).gap(12).items_center()
      .border_b(1).border_color(t.border)
      .child(v_flex().flex_1().min_w(0).gap(2)
        .child(div().text_size(15).font_semibold().text_color(t.foreground).child('Subscription limits'))
        .child(headerMeta))
      .child(closeBtn);

    const body = v_flex().id('usage-body').flex_1().min_h(0).min_w(0)
      .px(pad).py(9).gap(7)
      .overflow_y_scrollbar().track_scroll(this.scroll).track_focus(this.scrollFocus).tab_index(1)
      .text_size(12.5).text_color(t.foreground)
      .on_key_down((e, cx) => {
        if (e.key === 'down' || e.key === 'pagedown') this.scrollIndex = Math.min(maxScroll, this.scrollIndex + 1);
        else if (e.key === 'up' || e.key === 'pageup') this.scrollIndex = Math.max(0, this.scrollIndex - 1);
        else if (e.key === 'home') this.scrollIndex = 0;
        else if (e.key === 'end') this.scrollIndex = maxScroll;
        else return;
        this.scroll.scroll_to_item(this.scrollIndex);
        cx.stop_propagation();
      });

    if (!snapshot) {
      body.child(div().py(16).text_size(13).text_color(t.muted).child(this.failed ? 'Usage unavailable' : 'Reading limits…'));
    } else {
      for (const v of all) {
        const p = v.provider;
        const reportTime = p.fetched_at_ms === null ? '' : `Report ${formatTime(p.fetched_at_ms)}`;
        const regionLabel = `${v.name}${reportTime ? `, ${reportTime}` : ''}${p.stale ? ', stale' : ''}`;
        const sectionHeader = h_flex().gap(8).items_center().justify_between().min_w(0)
          .child(h_flex().gap(6).items_center().min_w(0)
            .child(div().text_size(12.5).font_semibold().text_color(t.foreground).child(v.name))
            .when(p.stale && p.accounts.length > 0, el => el.child(div().text_size(11).text_color(t.working).child('Stale'))));
        if (reportTime) {
          sectionHeader.child(mono(div().flex_shrink_0().text_size(11).text_color(t.muted).child(reportTime)));
        }

        const section = v_flex().id(`usage-provider-${p.provider}`).flex_shrink_0().min_w(0)
          .px(narrow ? 10 : 12).py(narrow ? 7 : 8).gap(6)
          .rounded(7).border(1).border_color(t.border).bg(t.recessed)
          .role('region').accessibility_label(regionLabel)
          .child(sectionHeader);

        if (p.error) {
          section.child(div().text_size(11.5).text_color(t.working).child(`Last check failed: ${errorText(p.error, p.provider)}`));
        }
        if (!p.accounts.length) {
          section.child(div().text_size(12).text_color(t.muted).child(providerStatusText(p)));
        }
        p.accounts.forEach((a, i) => {
          if (p.accounts.length > 1) {
            const acc = div().text_size(11.5).font_semibold().text_color(t.secondary).child(`Account ${i + 1} · ${formatDateTime(a.fetched_at_ms)}`);
            if (i > 0) acc.pt(5).border_t(1).border_color(t.border);
            section.child(acc);
          }
          if (!a.limits.length) {
            section.child(div().text_size(12).text_color(t.muted).child('No usage report available'));
          }
          for (const l of a.limits) {
            section.child(this.detailLimit(l, a, p, v.name, i, offline, t, now, narrow));
          }
        });
        body.child(section);
      }
    }

    const activeCount = snapshot ? all.filter(v => v.provider.state !== 'not_signed_in').length : 0;
    const footer = h_flex().flex_shrink_0().px(pad).py(8).gap(8).items_center().justify_between()
      .border_t(1).border_color(t.border)
      .child(div().flex_1().min_w(0).text_size(11).text_color(t.muted).child('Usage changed elsewhere may appear after a later check.'));
    if (!narrow && activeCount > 0) {
      footer.child(div().flex_shrink_0().text_size(11).text_color(t.muted).child(`${activeCount} ${activeCount === 1 ? 'provider' : 'providers'}`));
    }

    return v_flex().id('usage-details').size_full().min_h(0).min_w(0)
      .bg(t.popover).text_color(t.foreground).text_size(12.5)
      .role('dialog').accessibility_label('Subscription limits')
      .child(header)
      .child(body)
      .child(footer);
  }

  render() {
    const snapshot = readShared(), offline = !validated || this.failed, t = palette(), now = Date.now();
    return div().id('usage-root').size_full().min_w(0).min_h(0)
      .on_size_change((size, cx) => {
        if (this.width !== size.width) {
          this.width = size.width;
          cx.notify();
        }
      })
      .child(this.details ? this.detail(snapshot, offline, t, now) : this.statusbar(snapshot, offline, t, now));
  }
}
