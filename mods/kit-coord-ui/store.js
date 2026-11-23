import { exec } from 'herdr';
import { BINARY, DATA_DIR } from './runtime.js';
import { decode, decodeRoles, decodeArtifacts, decodeArtifactText, decodeArtifactPreview, parseCliError, sayArgs } from './model.js';
import { stagePreview, evictPreview, cleanupAllPreviews } from './preview.js';

const prefix = () => DATA_DIR ? ['--data-dir', DATA_DIR] : [];
const firstLine = error => String(error?.message || error || 'Unavailable').split('\n')[0].slice(0,240);
export class RailStore {
  constructor() {
    this.status = null; this.log = null; this.board = null; this.conversation = null;
    this.reading = false; this.sending = false; this.error = ''; this.notice = '';
    this.journal = null; this.loadingOlder = false; this.journalError = '';
    this.roles = null; this.loadingRoles = false; this.savingRole = false;
    this.rolesError = ''; this.rolesNotice = ''; this.rolesNoticeUntil = 0;
    this.wsArtifacts = new Map();
    this.artifactViewer = null;
    this.imagePreviews = new Map();
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
        // A plain confirmation fades; anything that needs reading stays.
        this.noticeUntil = outcome.coordinator_changed ? 0 : Date.now()+6000;
        view.follow = true;
      } catch (error) { this.notice = firstLine(error); this.noticeUntil = 0; }
      finally { this.sending = false; this.pending = null; this.refresh(cx,view); if (!view.closed) cx.notify(); }
    });
  }
  async readRoles() {
    const result = await exec(BINARY,[...prefix(),'roles'],{timeout:20000});
    if (result.code !== 0) throw new Error(parseCliError(result.stderr,'Could not read roles'));
    return decodeRoles(result.stdout);
  }
  loadRoles(cx, view) {
    if (this.loadingRoles || view.closed) return;
    this.loadingRoles = true; this.rolesError = ''; cx.notify();
    cx.spawn(async cx => {
      try {
        const roles = await this.readRoles();
        if (view.closed) return;
        this.roles = roles; this.rolesError = '';
        view.didLoadRoles?.(cx);
      } catch (error) { this.rolesError = firstLine(error); }
      finally { this.loadingRoles = false; if (!view.closed) cx.notify(); }
    });
  }
  mutateRole(cx, view, argv, {notice = '', onSuccess} = {}) {
    if (!argv || this.savingRole || view.closed) return;
    this.savingRole = true; this.rolesError = ''; this.rolesNotice = ''; cx.notify();
    cx.spawn(async cx => {
      try {
        const result = await exec(BINARY,[...prefix(),...argv],{timeout:20000});
        if (result.code !== 0) throw new Error(parseCliError(result.stderr,'Role update failed'));
        const outcome = JSON.parse(result.stdout);
        const roles = await this.readRoles();
        if (view.closed) return;
        this.roles = roles; this.rolesError = '';
        view.didLoadRoles?.(cx);
        this.rolesNotice = notice; this.rolesNoticeUntil = notice ? Date.now()+5000 : 0;
        if (onSuccess) onSuccess(outcome, cx);
      } catch (error) { this.rolesError = firstLine(error); }
      finally { this.savingRole = false; if (!view.closed) cx.notify(); }
    });
  }
  wsArtifactMap(wsId) {
    return this.wsArtifacts.get(wsId)?.byId || null;
  }
  loadWorkstreamArtifacts(cx, view, wsId) {
    if (!wsId || view.closed) return;
    const cur = this.wsArtifacts.get(wsId);
    if (cur?.loading) return;
    this.wsArtifacts.set(wsId, { items: cur?.items || [], byId: cur?.byId || new Map(), loading: true, error: '' });
    cx.notify();
    cx.spawn(async cx => {
      try {
        const result = await exec(BINARY, [...prefix(), 'artifacts', 'list', '--ws', wsId, '--json'], { timeout: 15000 });
        if (view.closed) return;
        if (result.code !== 0) throw new Error(parseCliError(result.stderr, 'Could not inspect attachments'));
        const items = decodeArtifacts(result.stdout);
        const byId = new Map(items.map(item => [item.id, item]));
        this.wsArtifacts.set(wsId, { items, byId, loading: false, error: '' });
      } catch (error) {
        if (view.closed) return;
        const prev = this.wsArtifacts.get(wsId);
        this.wsArtifacts.set(wsId, { items: prev?.items || [], byId: prev?.byId || new Map(), loading: false, error: firstLine(error) });
      } finally { if (!view.closed) cx.notify(); }
    });
  }
  openTextViewer(cx, view, att) {
    if (!att?.id || view.closed) return;
    if (view.page !== 'artifact') view.artifactReturnPage = view.page;
    view.page = 'artifact';
    view.confirmHtmlId = null;
    this.artifactViewer = {
      attachment: att, loading: true, text: '', truncated: false,
      total_bytes: att.bytes || 0, shown_bytes: 0, cap_bytes: 65536,
      resolved_path: att.resolved_path || att.source_path || '', error: '',
    };
    cx.notify();
    cx.spawn(async cx => {
      try {
        const result = await exec(BINARY, [...prefix(), 'artifact', 'read', att.id, '--max-bytes', '65536', '--json'], { timeout: 15000 });
        if (view.closed || this.artifactViewer?.attachment?.id !== att.id) return;
        if (result.code !== 0) throw new Error(parseCliError(result.stderr, 'Could not read attachment'));
        const payload = decodeArtifactText(result.stdout);
        this.artifactViewer = {
          attachment: { ...att, resolved_path: payload.resolved_path || att.resolved_path },
          loading: false, text: payload.text, truncated: !!payload.truncated,
          total_bytes: payload.total_bytes ?? 0, shown_bytes: payload.shown_bytes ?? 0,
          cap_bytes: payload.cap_bytes ?? 65536, resolved_path: payload.resolved_path || att.resolved_path || '', error: '',
        };
      } catch (error) {
        if (view.closed || this.artifactViewer?.attachment?.id !== att.id) return;
        this.artifactViewer = { ...this.artifactViewer, loading: false, error: firstLine(error) };
      } finally { if (!view.closed) cx.notify(); }
    });
  }
  toggleImagePreview(cx, view, att) {
    if (!att?.id || view.closed) return;
    const existing = this.imagePreviews.get(att.id);
    if (existing && (existing.status === 'ready' || existing.status === 'loading')) {
      this.imagePreviews.delete(att.id);
      if (existing.cacheKey) cx.spawn(async () => { await evictPreview(existing.cacheKey); });
      cx.notify();
      return;
    }
    this.imagePreviews.set(att.id, { status: 'loading' });
    cx.notify();
    cx.spawn(async cx => {
      try {
        const res = await exec(BINARY, [...prefix(), 'artifact', 'preview', att.id, '--json'], { timeout: 15000 });
        if (view.closed || this.imagePreviews.get(att.id)?.status !== 'loading') return;
        if (res.code !== 0) throw new Error(parseCliError(res.stderr, 'Image preview unavailable'));
        const info = decodeArtifactPreview(res.stdout);
        const relPath = await stagePreview(info, async () => {
          const inlineRes = await exec(BINARY, [...prefix(), 'artifact', 'preview', att.id, '--inline-bytes', '--json'], { timeout: 15000 });
          if (inlineRes.code !== 0) throw new Error(parseCliError(inlineRes.stderr, 'Could not read image bytes'));
          return decodeArtifactPreview(inlineRes.stdout).data_base64;
        });
        if (view.closed || this.imagePreviews.get(att.id)?.status !== 'loading') {
          await evictPreview(info.cache_key);
          return;
        }
        this.imagePreviews.set(att.id, {
          status: 'ready', relPath, width: info.width, height: info.height,
          format: info.format, bytes: info.bytes, cacheKey: info.cache_key,
        });
      } catch (error) {
        if (view.closed) return;
        this.imagePreviews.set(att.id, { status: 'error', error: firstLine(error) });
      } finally { if (!view.closed) cx.notify(); }
    });
  }
  openAttachment(cx, view, att, { allowHtml = false } = {}) {
    if (!att?.id || view.closed) return;
    view.confirmHtmlId = null;
    cx.notify();
    cx.spawn(async cx => {
      try {
        const args = [...prefix(), 'artifact', 'open', att.id, ...(allowHtml ? ['--allow-html'] : [])];
        const res = await exec(BINARY, args, { timeout: 15000 });
        if (view.closed) return;
        if (res.code !== 0) throw new Error(parseCliError(res.stderr, `Could not open ${att.label}`));
        this.notice = `Opened ${att.label}.`;
        this.noticeUntil = Date.now() + 4000;
      } catch (error) {
        if (view.closed) return;
        this.notice = firstLine(error);
        this.noticeUntil = 0;
      } finally { if (!view.closed) cx.notify(); }
    });
  }
  revealAttachment(cx, view, att) {
    if (!att?.id || view.closed) return;
    cx.spawn(async cx => {
      try {
        const res = await exec(BINARY, [...prefix(), 'artifact', 'reveal', att.id], { timeout: 15000 });
        if (view.closed) return;
        if (res.code !== 0) throw new Error(parseCliError(res.stderr, `Could not reveal ${att.label}`));
        this.notice = `Revealed ${att.label}.`;
        this.noticeUntil = Date.now() + 4000;
      } catch (error) {
        if (view.closed) return;
        this.notice = firstLine(error);
        this.noticeUntil = 0;
      } finally { if (!view.closed) cx.notify(); }
    });
  }
  copyAttachmentPath(cx, view, att) {
    const path = att?.resolved_path || att?.source_path || '';
    if (!path) return;
    cx.write_to_clipboard?.(path);
    this.notice = `Copied path for ${att.label || 'attachment'}.`;
    this.noticeUntil = Date.now() + 4000;
    cx.notify();
  }
  disposePreviews() {
    this.imagePreviews.clear();
    return cleanupAllPreviews();
  }
}
