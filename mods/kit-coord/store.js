import { exec, watchFolder, shared, setShared, notify } from 'herdr';
import { BINARY, DATA_DIR } from './runtime.js';
import { decodeStatus, decodeBoard, decodeLog, shouldAutostart, mergeTimelinePage, mergeRefreshPage, logArgs, timelineModel, shouldDeferTimeline, mergePendingEntries } from './model.js';

const KEY = 'herdr-kit.coord.status.v1';
const prefix = () => DATA_DIR ? ['--data-dir', DATA_DIR] : [];
const firstLine = e => String(e || 'unavailable').split('\n')[0].slice(0, 160);
let autostartTried = false;
let sharedRead = false;
const pending = { status: false, board: false, log: false };
const dirty = { status: false, board: false, log: false };
const scheduled = { status: false, board: false, log: false };
const queued = { status: {}, board: {}, log: {} };
const last = { status: 0, board: 0, log: 0 };
let generation = 0, statusRevision = 0;

export const store = {
  status: null, board: null, log: null, failed: false, error: '', failures: 0,
  boardFailed: false, logFailed: false, boardError: '', logError: '', starting: false, scope: {}, logScope: '', overlay: null,
  newEntries: [], newCount: 0, newGap: false,
  restartPending: false, restarting: false, restartTerminal: null, restartMessage: '',
  init() {
    if (sharedRead) return;
    sharedRead = true;
    try { const value = shared(KEY); if (value) { this.status = decodeStatus(JSON.stringify(value)); this.restartPending = !!this.status.coordinator?.restart_pending; } } catch (_) {}
  },
  refresh(cx, kind, force = false, options = {}) {
    if (kind !== 'status' && (!this.overlay || this.overlay.closed || (kind === 'board' ? this.overlay.ui.view !== 'board' : this.overlay.ui.view !== 'timeline'))) return;
    if (pending[kind]) { dirty[kind] = true; queued[kind] = options; return; }
    const delay = 750 - (Date.now() - last[kind]);
    if (!force && delay > 0) {
      dirty[kind] = true;
      queued[kind] = options;
      if (!scheduled[kind]) {
        scheduled[kind] = true;
        cx.spawn(async cx => { await cx.sleep(delay); scheduled[kind] = false; if (dirty[kind]) this.refresh(cx, kind, false, queued[kind]); });
      }
      return;
    }
    dirty[kind] = false; pending[kind] = true; last[kind] = Date.now();
    const scopeKey = JSON.stringify(this.scope || {});
    const requestGeneration = generation;
    const failedBefore = this.failed;
    let mode = options.mode || 'replace';
    let argv = [kind, '--json'];
    if (kind === 'log') {
      if (scopeKey !== this.logScope) mode = 'replace';
      const page = { limit: 200 };
      if (mode === 'append' && this.log?.oldest_seq != null) page.beforeSeq = this.log.oldest_seq;
      if (mode === 'prepend' && this.log?.newest_seq != null) page.afterSeq = this.newEntries[0]?.seq ?? this.log.newest_seq;
      argv = ['log', ...logArgs(this.scope || {}, page)];
    }
    cx.spawn(async cx => {
      try {
        let result;
        try { result = await exec(BINARY, [...prefix(), ...argv], {timeout: kind === 'status' ? 10000 : 15000}); }
        catch (e) {
          const message = firstLine(e?.message || e);
          throw new Error(/timed?\s*out|timeout/i.test(message) ? `coord ${kind} timed out` : `Could not run kit-coord: ${message}`);
        }
        if (result.code !== 0) throw new Error(`coord ${kind} failed (exit ${result.code}): ${firstLine(result.stderr)}`);
        let value;
        try { value = kind === 'status' ? decodeStatus(result.stdout) : kind === 'board' ? decodeBoard(result.stdout) : decodeLog(result.stdout); }
        catch (_) { throw new Error(`Unexpected ${kind === 'log' ? 'journal' : kind} format. Update kit-coord and reload the mod.`); }
        let refreshGap = false;
        if (kind === 'log' && mode === 'prepend' && this.log?.newest_seq != null) {
          const afterSeq = this.newEntries[0]?.seq ?? this.log.newest_seq;
          let pages = mergeRefreshPage({}, value);
          while (pages.beforeSeq != null && requestGeneration === generation) {
            let next;
            try { next = await exec(BINARY, [...prefix(), 'log', ...logArgs(this.scope || {}, {limit:200,afterSeq,beforeSeq:pages.beforeSeq})], {timeout:15000}); }
            catch (e) { const message=firstLine(e?.message || e); throw new Error(/timed?\s*out|timeout/i.test(message) ? 'coord log timed out' : `Could not run kit-coord: ${message}`); }
            if (next.code !== 0) throw new Error(`coord log failed (exit ${next.code}): ${firstLine(next.stderr)}`);
            let page;
            try { page=decodeLog(next.stdout); }
            catch (_) { throw new Error('Unexpected journal format. Update kit-coord and reload the mod.'); }
            pages=mergeRefreshPage(pages,page);
          }
          refreshGap=pages.gap;
          value={...pages.latest,entries:pages.entries};
        }
        if (kind === 'status') {
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
        } else if (kind === 'board') { this.board = value; this.boardFailed = false; this.boardError = ''; }
        else if (requestGeneration === generation && scopeKey === JSON.stringify(this.scope || {})) {
          if (mode === 'replace' || !this.log || this.logScope !== scopeKey) {
            this.log = value; this.newEntries = []; this.newCount = 0; this.newGap = false;
          } else if (mode === 'prepend' && (this.newCount || (this.overlay && !this.overlay.closed && shouldDeferTimeline(timelineModel(this.log, this.overlay.ui).rows, this.overlay.ui.selectedKey, this.newEntries)))) {
            // ScrollHandle cannot report its offset; selection away from the first row stands in for scrolling down.
            const previousCount = this.newEntries.length;
            this.newEntries = mergePendingEntries(this.newEntries, value.entries, this.log.entries);
            this.newGap = this.newGap || refreshGap || previousCount + value.entries.length > 2000;
            this.newCount = this.newEntries.length;
          } else {
            this.log = mergeTimelinePage(this.log,value,mode,refreshGap);
          }
          this.logScope = scopeKey; this.logFailed = false; this.logError = '';
        }
      } catch (e) {
        const message = firstLine(e?.message || e);
        if (kind === 'status') { this.error = message; this.failed = true; this.failures++; }
        else { this[kind + 'Error'] = message; this[kind + 'Failed'] = true; }
      } finally {
        pending[kind] = false; cx.notify();
        if (kind === 'status' && (this.failed || failedBefore)) setShared('herdr-kit.coord.status-revision', ++statusRevision);
        if (dirty[kind]) this.refresh(cx, kind, false, queued[kind]);
      }
    });
  },
  setScope(scope) { this.scope = scope || {}; generation++; this.log = null; this.newEntries = []; this.newCount = 0; this.newGap = false; },
  acceptNew() {
    if (!this.log || !this.newCount) return;
    this.log = mergeTimelinePage(this.log,{...this.log,entries:this.newEntries},'prepend',this.newGap);
    this.newEntries = []; this.newCount = 0; this.newGap = false;
  },
  up(cx, automatic = false) {
    if (this.starting || this.status?.state === 'running') return;
    this.starting = true; cx.notify();
    cx.spawn(async cx => {
      try {
        const result = await exec(BINARY, [...prefix(), 'up'], {timeout: 60000});
        if (result.code !== 0) throw new Error(firstLine(result.stderr));
      } catch (e) { notify({title: 'Coordinator did not start', body: firstLine(e?.message || e), level: automatic ? 'warning' : 'error', timeout: 8000}); }
      finally { this.starting = false; this.refresh(cx, 'status', true); if (this.overlay?.ui.view === 'board') this.refresh(cx, 'board', true); cx.notify(); }
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
        if (view.details) this.refresh(cx, view.ui.view === 'board' ? 'board' : 'log', false, {mode:'prepend'});
      }
    });
  }
};
