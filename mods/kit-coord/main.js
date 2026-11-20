import { View, div } from 'gpui-kit';
import { props, nextCommand, run, setShared } from 'herdr';
import { MOD_NAME } from './runtime.js';
import { store } from './store.js';
import { statusbarModel } from './model.js';
import { renderStatus } from './ui-status.js';

let strips = 0, commandRevision = 0;
export default class Coordinator extends View {
  init(_p, cx) {
    this.source = props()?.view === 'source';
    this.closed = false; this.width = 280; this.store = store;
    this.focus = cx.focus_handle();
    store.init();
    this.stripOwner = strips++ === 0;
    if (this.stripOwner) {
      store.refresh(cx, 'status', true); store.watch(cx, this);
      cx.timer.every(20000, cx => { if (!this.closed) store.refresh(cx, 'status'); });
      cx.timer.every(60000, cx => cx.notify());
      cx.spawn(async cx => {
        while (!this.closed) {
          const command = await nextCommand();
          if (this.closed) break;
          if (command === 'start-coordinator') store.up(cx);
          else if (command === 'reread-status') store.refresh(cx, 'status', true);
          else if (command === 'clear-context') store.restart(cx);
          else if (command === 'restart-now') store.restart(cx, true);
          else if (command === 'restart' && !store.restartPending) store.restart(cx);
          setShared('herdr-kit.coord.command', ++commandRevision);
        }
      });
    }
  }
  close() { this.closed = true; strips = Math.max(0, strips - 1); }
  start(cx) { store.up(cx); }
  refresh(cx) { store.refresh(cx, 'status', true); }
  focusCoordinator() {
    const terminal = store.status?.coordinator?.terminal;
    if (terminal) run('agent.focus', {terminal});
  }
  render() {
    const vm = statusbarModel(store.status, {width:80, now:Date.now(), starting:store.starting, failed:store.failed, failures:store.failures, lastOkMs:store.status?.generated_at_ms, error:store.error});
    const readout = {schema_version:1, mod:MOD_NAME, ...vm, icon_path:`mods/${MOD_NAME}/${vm.icon}`};
    const encoded = JSON.stringify(readout);
    if (encoded !== this.published) { this.published = encoded; setShared('herdr-kit.coord.readout.v1',readout); }
    if (this.source) return div().w(0).h(0);
    return div().id('coord-root').size_full().on_size_change((size,cx) => { if (this.width !== size.width) { this.width = size.width; cx.notify(); } }).child(renderStatus(this));
  }
}
