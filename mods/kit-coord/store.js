import { exec, watchFolder, shared, setShared, notify } from 'herdr';
import { BINARY, DATA_DIR } from './runtime.js';
import { decodeStatus, shouldAutostart } from './model.js';

const KEY = 'herdr-kit.coord.status.v1';
const prefix = () => DATA_DIR ? ['--data-dir', DATA_DIR] : [];
const firstLine = e => String(e || 'unavailable').split('\n')[0].slice(0, 160);
let autostartTried = false;
let sharedRead = false;
let pending = false, dirty = false, scheduled = false, last = 0, statusRevision = 0;

export const store = {
  status: null, failed: false, error: '', failures: 0, starting: false,
  restartPending: false, restarting: false, restartTerminal: null, restartMessage: '',
  init() {
    if (sharedRead) return;
    sharedRead = true;
    try { const value = shared(KEY); if (value) { this.status = decodeStatus(JSON.stringify(value)); this.restartPending = !!this.status.coordinator?.restart_pending; } } catch (_) {}
  },
  refresh(cx, _kind, force = false) {
    if (pending) { dirty = true; return; }
    const delay = 750 - (Date.now() - last);
    if (!force && delay > 0) {
      dirty = true;
      if (!scheduled) {
        scheduled = true;
        cx.spawn(async cx => { await cx.sleep(delay); scheduled = false; if (dirty) this.refresh(cx, 'status'); });
      }
      return;
    }
    dirty = false; pending = true; last = Date.now();
    const failedBefore = this.failed;
    cx.spawn(async cx => {
      try {
        let result;
        try { result = await exec(BINARY, [...prefix(), 'status', '--json'], {timeout: 10000}); }
        catch (e) {
          const message = firstLine(e?.message || e);
          throw new Error(/timed?\s*out|timeout/i.test(message) ? 'coord status timed out' : `Could not run kit-coord: ${message}`);
        }
        if (result.code !== 0) throw new Error(`coord status failed (exit ${result.code}): ${firstLine(result.stderr)}`);
        let value;
        try { value = decodeStatus(result.stdout); }
        catch (_) { throw new Error('Unexpected status format. Update kit-coord and reload the mod.'); }
        const previous = this.status;
        const terminal = value.coordinator?.terminal;
        const before = this.restartTerminal || (this.restartPending ? previous?.coordinator?.terminal : null);
        this.restartPending = !!value.coordinator?.restart_pending;
        if (this.restartPending) {
          this.restartTerminal = before || terminal;
          this.restartMessage = value.state === 'running' ? 'Will restart when idle.' : 'Waiting for the new coordinator session…';
        } else if (before && terminal && terminal !== before && value.state === 'running') {
          this.restartTerminal = null; this.restartMessage = 'Coordinator restarted.';
        } else if (before && value.state !== 'running') {
          this.restartMessage = 'Restart not yet verified. Check coordinator health below.';
        } else if (value.state !== 'running') this.restartMessage = '';
        this.status = value; this.failed = false; this.error = ''; this.failures = 0;
        const comparable = s => { if (!s) return ''; const { generated_at_ms, ...rest } = s; return JSON.stringify(rest); };
        if (comparable(previous) !== comparable(value)) setShared(KEY, value);
        if (shouldAutostart(value, autostartTried)) { autostartTried = true; this.up(cx, true); }
      } catch (e) {
        this.error = firstLine(e?.message || e); this.failed = true; this.failures++;
      } finally {
        pending = false; cx.notify();
        if (this.failed || failedBefore) setShared('herdr-kit.coord.status-revision', ++statusRevision);
        if (dirty) this.refresh(cx, 'status');
      }
    });
  },
  up(cx, automatic = false) {
    if (this.starting || this.status?.state === 'running') return;
    this.starting = true; cx.notify();
    cx.spawn(async cx => {
      try {
        const result = await exec(BINARY, [...prefix(), 'up'], {timeout: 60000});
        if (result.code !== 0) throw new Error(firstLine(result.stderr));
      } catch (e) { notify({title: 'Coordinator did not start', body: firstLine(e?.message || e), level: automatic ? 'warning' : 'error', timeout: 8000}); }
      finally { this.starting = false; this.refresh(cx, 'status', true); cx.notify(); }
    });
  },
  restart(cx, force = false) {
    if (this.restarting || !this.status?.enabled || this.status.state !== 'running' || (force && !this.restartPending)) return;
    this.restarting = true; this.restartMessage = ''; const terminal = this.status.coordinator?.terminal; cx.notify();
    cx.spawn(async cx => {
      try {
        const result = await exec(BINARY, [...prefix(), 'restart', ...(force ? ['--force'] : [])], {timeout: 60000});
        if (result.code !== 0) throw new Error(firstLine(result.stderr));
        const value = JSON.parse(result.stdout);
        if (value.state !== 'requested' && value.state !== 'restarted') throw new Error('Unexpected restart response');
        this.restartPending = value.state === 'requested'; this.restartTerminal = terminal;
        this.restartMessage = this.restartPending ? 'Will restart when idle.' : 'Checking new coordinator session…';
      } catch (e) { this.restartMessage = `Could not clear context: ${firstLine(e?.message || e)}`; }
      finally { this.restarting = false; this.refresh(cx, 'status', true); cx.notify(); }
    });
  },
  watch(cx, view) {
    cx.spawn(async cx => {
      while (!view.closed) {
        const dir = this.status?.journal_dir || (DATA_DIR ? DATA_DIR + '/coord/journal' : null);
        if (!dir) { await cx.sleep(30000); continue; }
        try { await watchFolder(dir); }
        catch (_) { await cx.sleep(30000); continue; }
        if (view.closed) break;
        this.refresh(cx, 'status');
      }
    });
  }
};
