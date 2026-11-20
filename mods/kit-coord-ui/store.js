import { exec } from 'herdr';
import { BINARY, DATA_DIR } from './runtime.js';
import { decode, sayArgs } from './model.js';

const prefix = () => DATA_DIR ? ['--data-dir', DATA_DIR] : [];
const firstLine = error => String(error?.message || error || 'Unavailable').split('\n')[0].slice(0,240);
export class RailStore {
  constructor() {
    this.status = null; this.log = null; this.board = null; this.conversation = null;
    this.reading = false; this.sending = false; this.error = ''; this.notice = '';
    this.journal = null; this.loadingOlder = false; this.journalError = '';
  }
  async read(kind, beforeSeq = null) {
    const result = await exec(BINARY,[...prefix(),kind,'--json',...(kind === 'board' ? ['--all'] : []),...(kind === 'log' ? ['--limit','200',...(beforeSeq == null ? [] : ['--before-seq',String(beforeSeq)])] : [])],{timeout:20000});
    if (result.code !== 0) throw new Error(result.stderr || `coord ${kind} is unavailable`);
    return decode(result.stdout,kind);
  }
  refresh(cx, view) {
    if (this.reading || view.closed) return;
    this.reading = true;
    cx.spawn(async cx => {
      try {
        const [status,conversation,log,board] = await Promise.all(['status','conversation','log','board'].map(kind => this.read(kind)));
        if (view.closed) return;
        if ((status.coordinator?.terminal ?? null) !== (conversation.coordinator?.terminal ?? null)) throw new Error('Coordinator session is changing. Refreshing…');
        this.status = status; this.conversation = conversation; this.log = log; this.board = board; this.error = '';
        if (!this.journal) this.journal = log;
        view.didRefresh(cx);
      } catch (error) {
        // Keep the last history readable but never enable sending from a stale status.
        const message = firstLine(error), changed = this.error !== message;
        this.error = message;
        if (changed && !view.closed) view.latest(cx);
      } finally { this.reading = false; if (!view.closed) cx.notify(); }
    });
  }
  loadOlder(cx, view) {
    if (this.loadingOlder || !this.journal?.has_more || view.closed) return;
    const entries = this.journal.entries;
    const before = entries.reduce((min, entry) => Math.min(min, entry.seq), Infinity);
    if (!Number.isFinite(before)) return;
    this.loadingOlder = true; this.journalError = ''; cx.notify();
    cx.spawn(async cx => {
      try {
        const page = await this.read('log',before);
        if (view.closed) return;
        const seen = new Set();
        const merged = [...page.entries,...entries].sort((a,b) => a.seq-b.seq)
          .filter(entry => !seen.has(entry.seq) && seen.add(entry.seq));
        view.journalStart = Math.max(0,(view.journalStart || 0)-Math.max(0,merged.length-500));
        this.journal = {...page,entries:merged.slice(0,500)};
      } catch (error) { this.journalError = firstLine(error); }
      finally { this.loadingOlder = false; if (!view.closed) cx.notify(); }
    });
  }
  send(cx, view, option = null) {
    const argv = option ? sayArgs(option.text,option.question) : sayArgs(view.input.value(),view.question);
    if (!argv || this.sending || !view.model().canSend || view.model().working) return;
    this.sending = true; this.pending = {key:'pending-send',kind:'person',text:argv[2],sending:true,ts_ms:Date.now()}; this.notice = ''; cx.notify();
    cx.spawn(async cx => {
      try {
        const result = await exec(BINARY,[...prefix(),...argv],{timeout:100000});
        if (result.code !== 0) throw new Error(result.stderr || 'Message delivery was not confirmed');
        const outcome = JSON.parse(result.stdout);
        if (outcome.state !== 'sent') throw new Error('Message delivery was not confirmed. Check the raw coordinator before sending again.');
        // Clear only the successfully delivered draft; edits made in flight survive.
        if (!option && view.input.value() === argv[2]) { view.input.set_value(''); view.draft = ''; view.question = null; }
        this.notice = outcome.coordinator_changed
          ? 'Delivered to the previous session; the question was not marked answered.'
          : 'Message delivered to the coordinator.';
        view.follow = true;
      } catch (error) { this.notice = firstLine(error); }
      finally { this.sending = false; this.pending = null; this.refresh(cx,view); if (!view.closed) cx.notify(); }
    });
  }
}
