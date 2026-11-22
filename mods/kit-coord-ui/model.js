// The Coordinator dialog consumes coordinator CLI JSON only. No journal or session I/O here.
export function decode(text, kind) {
  const value = JSON.parse(text);
  if (!value || value.schema_version !== 1) throw new Error(`Unsupported coordinator ${kind} format`);
  if (kind === 'log' && !Array.isArray(value.entries)) throw new Error('Invalid coordinator journal');
  if (kind === 'conversation' && !Array.isArray(value.messages)) throw new Error('Invalid coordinator conversation');
  if (kind === 'board' && (!Array.isArray(value.workstreams) || !Array.isArray(value.open_questions))) throw new Error('Invalid coordinator board');
  return value;
}
export function clock(ms) {
  const date = new Date(ms);
  return Number.isFinite(date.getTime()) ? `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}` : '—';
}
export function elapsed(ms) {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
}
const text = value => typeof value === 'string' ? value : '';
export function linksFor(task, workstream) {
  const links = [];
  if (task?.terminal) links.push({kind:'terminal', label:task.terminal, target:task.terminal});
  if (workstream?.space?.id) links.push({kind:'space', label:workstream.space.name || workstream.repo?.name || 'Space', target:workstream.space.id, tab:task?.tab_id});
  return links;
}
export function focusAction(link) {
  if (link.kind === 'terminal') return [{name:'agent.focus', args:{terminal:link.target}}];
  if (link.kind === 'space') return [{name:'space.select', args:{id:link.target}}, ...(link.tab ? [{name:'tab.select', args:{id:link.tab}}] : [])];
  return [];
}
export function quickReply(row, option) {
  return row.kind === 'question' && !row.closed && row.options.includes(option)
    ? {text:option, question:{id:row.questionId, time:clock(row.ts_ms)}} : null;
}
export function sayArgs(value, question) {
  if (!value.trim()) return null;
  return ['say', '--text', value, ...(question?.id ? ['--question', question.id] : [])];
}
export const ROLE_HARNESSES = ['omp', 'agy', 'claude', 'codex', 'copilot'];
export const ROLE_SESSIONS = ['dir', 'preset', 'codex-rollout', 'agy-conversation', 'none'];
export const HARNESS_DEFAULTS = {
  omp: {
    start: 'omp --session-dir {session_dir} {brief}',
    resume: 'omp --session-dir {session_dir} --resume {session_id} {brief}',
    session: 'dir',
    trust: '',
  },
  agy: {
    start: 'agy -i {brief}',
    resume: 'agy --conversation {session_id} -i {brief}',
    session: 'agy-conversation',
    trust: `--settings '{"trustedWorkspaces":[{cwd_json}]}'`,
  },
  claude: {
    start: 'claude --session-id {session_id} {brief}',
    resume: 'claude --resume {session_id} {brief}',
    session: 'preset',
    trust: '',
  },
  codex: {
    start: 'codex {brief}',
    resume: 'codex resume {session_id} {brief}',
    session: 'codex-rollout',
    trust: `-c 'projects={ "{repo_root}" = { trust_level = "trusted" } }'`,
  },
  copilot: {
    start: 'copilot --session-id {session_id} -i {brief}',
    resume: 'copilot --resume {session_id} -i {brief}',
    session: 'preset',
    trust: '',
  },
};
export function decodeRoles(raw) {
  const value = JSON.parse(raw);
  if (!value || !Array.isArray(value.roles)) throw new Error('Invalid coordinator roles response');
  return value.roles;
}
export function parseCliError(stderr, fallback = 'Command failed') {
  const raw = String(stderr || '').trim();
  if (!raw) return fallback;
  try {
    const parsed = JSON.parse(raw);
    if (parsed?.error?.message) return String(parsed.error.message);
  } catch { /* fall through to plain text */ }
  return raw.split('\n')[0].slice(0, 240);
}
export function filterRoles(roles, query = '') {
  const q = oneLine(query).toLowerCase();
  if (!q) return roles || [];
  return (roles || []).filter(r =>
    [r.name, r.harness, r.model, r.effort, r.source, r.session]
      .some(v => String(v ?? '').toLowerCase().includes(q)));
}
export function roleSummaryLine(role) {
  return [
    role.harness || '',
    role.model || 'default',
    role.effort ? `effort: ${role.effort}` : '',
  ].filter(Boolean).join(' · ');
}
export function roleCreateArgs(form) {
  const name = String(form?.name || '').trim();
  if (!name) return null;
  const argv = ['roles', 'create', name];
  const from = String(form?.from || '').trim();
  if (from) argv.push('--from', from);
  const harness = String(form?.harness || '').trim();
  const model = String(form?.model || '').trim();
  const effort = String(form?.effort || '').trim();
  const session = String(form?.session || '').trim();
  const start = String(form?.start || '').trim();
  const resume = String(form?.resume || '').trim();
  const trust = String(form?.trust || '').trim();
  const body = String(form?.body ?? '');
  if (harness) argv.push('--harness', harness);
  if (model) argv.push('--model', model);
  else if (from) argv.push('--clear-model');
  if (effort) argv.push('--effort', effort);
  else if (from) argv.push('--clear-effort');
  if (session) argv.push('--session', session);
  if (start) argv.push('--start', start);
  if (resume) argv.push('--resume', resume);
  else if (from) argv.push('--clear-resume');
  if (trust) argv.push('--trust', trust);
  else if (from) argv.push('--clear-trust');
  argv.push('--prompt', body);
  return argv;
}
export function roleEditArgs(name, form) {
  const roleName = String(name || '').trim();
  if (!roleName) return null;
  const argv = ['roles', 'edit', roleName];
  const harness = String(form?.harness || '').trim();
  const model = String(form?.model || '').trim();
  const effort = String(form?.effort || '').trim();
  const session = String(form?.session || '').trim();
  const start = String(form?.start || '').trim();
  const resume = String(form?.resume || '').trim();
  const trust = String(form?.trust || '').trim();
  const body = String(form?.body ?? '');
  if (harness) argv.push('--harness', harness);
  if (model) argv.push('--model', model); else argv.push('--clear-model');
  if (effort) argv.push('--effort', effort); else argv.push('--clear-effort');
  if (session) argv.push('--session', session);
  if (start) argv.push('--start', start);
  if (resume) argv.push('--resume', resume); else argv.push('--clear-resume');
  if (trust) argv.push('--trust', trust); else argv.push('--clear-trust');
  argv.push('--prompt', body);
  return argv;
}
export function roleDeleteArgs(name) {
  const roleName = String(name || '').trim();
  return roleName ? ['roles', 'delete', roleName] : null;
}
export function roleResetArgs(name) {
  const roleName = String(name || '').trim();
  return roleName ? ['roles', 'reset', roleName] : null;
}
const oneLine = value => String(value ?? '').replace(/\s+/g,' ').trim();
const pad = n => String(n).padStart(2,'0');
const formatAge = ms => { const s=Math.max(0,Math.floor(ms/1000)); if(s<60) return `${s}s`; const m=Math.floor(s/60); if(m<60) return `${m}m`; const h=Math.floor(m/60); if(h<24) return `${h}h${m%60 ? ` ${m%60}m` : ''}`; return `${Math.floor(h/24)}d${h%24 ? ` ${h%24}h` : ''}`; };

// Journal sentences: plain language for the person; ids, tiers, rungs and paths
// stay in the expanded fields. Kept local so kit-coord-ui depends on the CLI
// contract, not another mod.
const SYSTEM_KINDS = /^(watcher\.|inbox\.|nudge\.|res\.|coord\.(turn|session_started|session_ended|handoff_requested)$|task\.(turn|launch_details|session)$)/;
export const isSystemEvent = e => SYSTEM_KINDS.test(e.kind);
export function journalEntry(e) {
  const d = e.data ?? {}, s = k => oneLine(typeof d[k] === 'string' || typeof d[k] === 'number' ? d[k] : '');
  const said = (prefix, value) => value ? `${prefix}: ${value}` : prefix;
  let tone = 'quiet', text;
  switch (e.kind) {
    case 'ws.opened': text = said('Opened', s('title')); break;
    case 'ws.closed': tone = 'done'; text = said('Closed', s('summary')); break;
    case 'ws.cleaned': text = 'Cleaned up'; break;
    case 'ws.retained': tone = 'blocked'; text = said('Kept open', s('reason')); break;
    case 'task.placed': text = said(`Started task${s('title') ? ` “${s('title')}”` : ''}`, s('reason')); break;
    case 'task.started': text = `Agent started${s('tab_name') ? ` in “${s('tab_name')}”` : ''}`; break;
    case 'task.progress': text = s('summary') || 'Progress'; break;
    case 'task.reported':
      tone = d.outcome === 'done' ? 'done' : d.outcome === 'question' ? 'question' : 'blocked';
      text = said(d.outcome === 'done' ? 'Done' : d.outcome === 'question' ? 'Asks' : 'Blocked', s('summary')); break;
    case 'task.closed': text = d.outcome && !['accepted','done','delivered'].includes(d.outcome) ? said('Task closed', s('reason') || s('outcome')) : 'Task closed'; break;
    case 'task.sent': text = 'Sent the agent a follow-up'; break;
    case 'task.note': text = said('Note', s('text')); break;
    case 'task.dormant': text = 'Agent ended; its session can be resumed'; break;
    case 'task.start_blocked': tone = 'blocked'; text = 'Agent did not start, probably a start-up dialog'; break;
    case 'task.stalled': tone = 'blocked'; text = 'No report after a nudge'; break;
    case 'decision': tone = d.tier === 'ask' ? 'question' : 'quiet'; text = said('Decided', s('what')); break;
    case 'question.asked': tone = 'question'; text = said('Asked you', s('text')); break;
    case 'question.answered': tone = 'done'; text = said('You answered', s('text')); break;
    case 'coord.sent': text = said('You', s('text')); break;
    case 'coord.session_started': text = 'Coordinator session started'; break;
    case 'coord.session_ended': text = said('Coordinator session ended', s('reason')); break;
    case 'coord.handoff_requested': text = 'Coordinator restart requested'; break;
    case 'instruction.set': text = said('Standing instruction', s('text')); break;
    case 'correction': tone = 'blocked'; text = said('You corrected', s('what')); break;
    case 'thread.opened': text = said('Opened thread', s('title')); break;
    case 'thread.closed': text = said('Closed thread', s('title')); break;
    case 'scout.result': text = said('Scout', s('summary')); break;
    case 'reconcile.finding': tone = 'blocked'; text = said('Check', s('summary')); break;
    case 'watcher.started': text = 'Watcher started'; break;
    case 'watcher.stopped': text = said('Watcher stopped', s('reason')); break;
    case 'nudge.sent': text = 'Nudged an agent'; break;
    case 'inbox.drained': text = 'Read the inbox'; break;
    case 'coord.turn': text = `Coordinator turn${typeof d.working_ms === 'number' ? ` (${formatAge(d.working_ms)})` : ''}`; break;
    case 'task.turn': text = `Agent turn${typeof d.working_ms === 'number' ? ` (${formatAge(d.working_ms)})` : ''}`; break;
    case 'task.launch_details': text = 'Recorded the agent launch'; break;
    case 'task.session': text = 'Recorded the agent session'; break;
    case 'res.created': case 'res.adopted': case 'res.released': case 'res.intent':
      text = `${({'res.created':'Created','res.adopted':'Adopted','res.released':'Released','res.intent':'Creating'})[e.kind]} ${[s('type'),s('name')].filter(Boolean).join(' ')}`.trim(); break;
    case 'res.release_checked': tone = d.ok === false ? 'blocked' : 'quiet'; text = `Release check ${s('check')} ${d.ok === false ? 'failed' : 'passed'}`.replace(/\s+/g,' '); break;
    default: text = said(e.kind, s('summary') || s('text') || s('title'));
  }
  const why = e.kind === 'decision' ? s('why') : e.kind === 'correction' ? s('why') : '';
  return {tone, text, why, system: isSystemEvent(e)};
}

const chronological = (a,b) => (a.ts_ms ?? 0)-(b.ts_ms ?? 0) || (a.order ?? 0)-(b.order ?? 0) || a.key.localeCompare(b.key);
const taskFor = (ws, id) => (ws?.tasks || []).find(t => t.id === id);
const questionItems = (board, entries) => (board?.open_questions || []).filter(q => q.qid).map(q => {
  const event = entries.find(e => e.kind === 'question.asked' && e.data?.qid === q.qid);
  const ws = (board?.workstreams || []).find(w => w.id === (q.ws || event?.ws));
  return {key:`question:${q.qid}`,kind:'question',questionId:q.qid,text:text(q.text || event?.data?.text),
    options:(q.options || event?.data?.options || []).filter(x => typeof x === 'string'),
    title:ws?.title || taskFor(ws,q.task || event?.task)?.title || 'Coordinator',
    ts_ms:q.since_ms ?? event?.ts_ms ?? 0,ws:q.ws || event?.ws,task:q.task || event?.task};
});
function needsYou(board, entries) {
  const questions = questionItems(board,entries), ids = new Set(questions.map(q => q.questionId));
  const attention = (board?.attention || []).filter(a => {
    if (a.kind === 'question') return false;
    if (a.qid && ids.has(a.qid)) return false;
    return !(a.kind === 'task_question' && questions.some(q =>
      a.ws && q.ws === a.ws && (!a.task || !q.task || a.task === q.task)));
  }).map(a => {
    const ws=(board?.workstreams || []).find(w => w.id===a.ws);
    return {key:`attention:${a.id}`,kind:'attention',text:text(a.text),options:[],title:ws?.title || taskFor(ws,a.task)?.title || 'Coordinator',
      ts_ms:a.since_ms ?? 0,ws:a.ws,task:a.task,source:a};
  });
  return [...questions,...attention].sort(chronological);
}
function traceRows(entries, ws) {
  return entries.filter(e => e.ws === ws.id && ['task.placed','task.started','task.progress','decision','task.reported','task.closed','ws.closed'].includes(e.kind))
    .map(e => {
      const d=e.data || {};
      const sentence = e.kind === 'task.placed' ? `Placed${d.reason ? ` — ${d.reason}` : ''}` :
        e.kind === 'task.started' ? `Started${ws.space?.name ? ` in ${ws.space.name}` : ''}` :
        e.kind === 'task.progress' ? `Progress: ${text(d.summary)}` :
        e.kind === 'decision' ? [d.what,d.why].filter(Boolean).join(' — ') :
        e.kind === 'task.reported' ? `Reported ${d.outcome || ''}${d.summary ? ` — ${d.summary}` : ''}` :
        `Closed${d.summary || d.reason ? ` — ${d.summary || d.reason}` : ''}`;
      return {key:`trace:${e.seq}`,ts_ms:e.ts_ms,text:sentence,source:e};
    }).sort(chronological);
}
function receiptFor(ws, entries, attention, now) {
  const events=entries.filter(e => e.ws === ws.id).sort((a,b) => (a.ts_ms-b.ts_ms) || (a.seq-b.seq));
  const tasks=[...(ws.tasks || [])].sort((a,b) => (b.last_event_ms || 0)-(a.last_event_ms || 0));
  const relevant=events.filter(e => ['task.progress','task.reported','ws.closed'].includes(e.kind)).at(-1);
  const task=tasks.find(t => ['running','placed'].includes(t.state)) || tasks.find(t => t.state === 'reported') || taskFor(ws,relevant?.task) || tasks[0];
  const closed=task && [...events].reverse().find(e => e.kind === 'task.closed' && e.task === task.id);
  const unsuccessful=task?.state === 'closed' && closed?.data?.outcome && !['accepted','done','delivered'].includes(closed.data.outcome);
  const item=attention.find(a => a.ws === ws.id && a.kind === 'question') || attention.find(a => a.ws === ws.id);
  const state=item?.kind === 'question' ? 'question' :
    item ? 'blocked' : ['closed','cleaned'].includes(ws.state) ? 'done' :
    unsuccessful ? 'blocked' : (task?.state === 'reported' && task.report === 'done') || task?.state === 'closed' ? 'done' :
    task?.state === 'reported' && task.report === 'blocked' ? 'blocked' :
    task?.start_blocked || task?.stalled ? 'blocked' : task?.state === 'placed' || !task ? 'placed' : 'running';
  const space=ws.space?.name || ws.repo?.name || '';
  const latestProgress=[...events].reverse().find(e => e.kind === 'task.progress' && (!task || e.task === task.id));
  const description=state === 'question' ? 'Waiting on you ↑' :
    state === 'blocked' ? item?.text || (unsuccessful ? closed.data.reason || closed.data.outcome : '') || task?.summary || 'Waiting for a blocked task' :
    state === 'done' ? ws.summary || (relevant?.kind === 'ws.closed' ? relevant.data?.summary : '') || task?.summary || relevant?.data?.summary || 'Done' :
    state === 'running' ? latestProgress?.data?.summary || task?.summary || `Agent started${space ? ` in ${space}` : ''}` :
    task?.placement_reason || 'Placed';
  return {key:`receipt:${ws.id}`,kind:'receipt',ws:ws.id,title:ws.title,text:description,state,ts_ms:ws.opened_ms ?? 0,
    opened_ms:ws.opened_ms ?? 0,space,duration:formatAge((ws.closed_ms ?? (state === 'done' ? task?.state_since_ms ?? task?.last_event_ms : null) ?? now)-(ws.opened_ms ?? now)),
    links:linksFor(task,ws),trace:traceRows(entries,ws),source:ws};
}
export function railRows(log, conversation, board, now = Date.now()) {
  const entries=log?.entries || [], attention=needsYou(board,entries), messages=[];
  for (const message of conversation?.messages || []) {
    if (!['user','assistant'].includes(message.role) || !text(message.text).trim()) continue;
    messages.push({key:`message:${message.id}`,kind:message.role === 'user' ? 'person' : 'reply',
      label:message.role === 'user' ? 'You' : 'Coordinator',ts_ms:message.ts_ms,text:message.text,links:[],source:message});
  }
  for (const e of entries) {
    if (e.kind !== 'coord.sent' || !text(e.data?.text).trim()) continue;
    const normalized=oneLine(e.data.text).toLowerCase();
    if (messages.some(m => m.kind === 'person' && oneLine(m.text).toLowerCase() === normalized && Math.abs(m.ts_ms-e.ts_ms) <= 120000)) continue;
    messages.push({key:`sent:${e.seq}`,kind:'person',ts_ms:e.ts_ms,text:e.data.text,links:[],source:e});
  }
  messages.sort(chronological);
  const workstreams=new Map((board?.workstreams || []).map(ws => [ws.id,ws]));
  for (const e of entries) {
    if (e.kind !== 'ws.opened' || !e.ws || workstreams.has(e.ws)) continue;
    const events=entries.filter(event => event.ws === e.ws);
    const close=events.filter(event => event.kind === 'ws.closed').sort((a,b) => b.ts_ms-a.ts_ms || b.seq-a.seq)[0];
    const report=events.filter(event => event.kind === 'task.reported' && text(event.data?.summary).trim()).sort((a,b) => b.ts_ms-a.ts_ms || b.seq-a.seq)[0];
    workstreams.set(e.ws,{id:e.ws,title:e.data?.title || 'Workstream',opened_ms:e.ts_ms,
      closed_ms:close?.ts_ms ?? null,state:close ? 'closed' : 'open',
      summary:close?.data?.summary || report?.data?.summary || null,tasks:[]});
  }
  const uniqueWorkstreams=[...workstreams.values()];
  const receipts=uniqueWorkstreams.map(ws => {
    const row=receiptFor(ws,entries,attention,now);
    const anchor=messages.filter(m => m.kind === 'person' && m.ts_ms <= row.opened_ms).at(-1);
    if (anchor && row.opened_ms-anchor.ts_ms <= 600000) { row.ts_ms=anchor.ts_ms; row.anchor=anchor.key; row.order=1; }
    return row;
  });
  const rows=[...messages,...receipts].sort(chronological);
  const latestReports=new Map();
  for (const e of entries) {
    if (e.kind !== 'task.reported' || !['done','blocked','question'].includes(e.data?.outcome)) continue;
    const previous=latestReports.get(e.ws);
    if (!previous || e.ts_ms > previous.ts_ms || (e.ts_ms === previous.ts_ms && e.seq > previous.seq)) latestReports.set(e.ws,e);
  }
  for (const [ws,e] of latestReports) {
    const receipt=receipts.find(r => r.ws === ws);
    if (!receipt || !messages.some(message => chronological(receipt,message) < 0 && message.ts_ms < e.ts_ms)) continue;
    rows.push({key:`echo:${e.seq}`,kind:'echo',receiptKey:receipt.key,ws:e.ws,title:receipt.title,
      text:text(e.data.summary),state:e.data.outcome === 'question' ? 'question' : e.data.outcome,ts_ms:e.ts_ms,order:2,source:e});
  }
  return rows.sort(chronological);
}
const dayKey = ms => { const d=new Date(ms); return `${d.getFullYear()}-${pad(d.getMonth()+1)}-${pad(d.getDate())}`; };
export function dayLabel(ms,now = Date.now()) {
  const d=new Date(ms), n=new Date(now), yesterday=new Date(n.getFullYear(),n.getMonth(),n.getDate()-1);
  return dayKey(ms)===dayKey(now) ? 'Today' : dayKey(ms)===dayKey(yesterday.getTime()) ? 'Yesterday' :
    `${['Sun','Mon','Tue','Wed','Thu','Fri','Sat'][d.getDay()]} ${['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'][d.getMonth()]} ${d.getDate()}${d.getFullYear() === n.getFullYear() ? '' : `, ${d.getFullYear()}`}`;
}
export function groupDays(rows, now = Date.now(), expanded = new Set()) {
  const groups=new Map();
  for (const row of rows) { const key=dayKey(row.ts_ms); if (!groups.has(key)) groups.set(key,[]); groups.get(key).push(row); }
  const result=[];
  for (const [day,items] of groups) {
    const collapsed=day<dayKey(now) && !expanded.has(day);
    const receipts=items.filter(r => r.kind === 'receipt'), count=receipts.length;
    const countLabel=`${count} ${count === 1 ? 'workstream' : 'workstreams'}`;
    const blocked=receipts.filter(r => r.state === 'blocked' || r.state === 'question').length;
    const summary=count && receipts.every(r => r.state === 'done') ? 'all done' : blocked ? `${blocked} blocked` : '';
    result.push({key:`day:${day}`,kind:'day',day,label:dayLabel(items[0].ts_ms,now),ts_ms:items[0].ts_ms,collapsible:day<dayKey(now),collapsed,count,countLabel,summary});
    if (!collapsed) result.push(...items);
  }
  return result;
}
// Newest first, grouped by day, then by consecutive runs of one workstream so
// its title is shown once instead of on every line.
export function journalRows(log, board, {filter = '', showSystem = false, now = Date.now()} = {}) {
  const entries = log?.entries || [];
  // A workstream opened before the loaded page still has a readable name from its tasks.
  const titles = new Map();
  for (const e of entries) if (e.ws && e.kind === 'task.placed' && e.data?.title && !titles.has(e.ws)) titles.set(e.ws, oneLine(e.data.title));
  for (const e of entries) if (e.ws && e.kind === 'ws.opened' && e.data?.title) titles.set(e.ws, oneLine(e.data.title));
  for (const ws of board?.workstreams || []) titles.set(ws.id, ws.title);
  const query = oneLine(filter).toLowerCase();
  const events = entries.filter(e => showSystem || !isSystemEvent(e)).map(e => {
    const title = e.ws ? titles.get(e.ws) || 'Earlier workstream' : 'Coordinator';
    return {key:`journal:${e.seq}`, kind:'event', ts_ms:e.ts_ms, ws:e.ws, title, ...journalEntry(e), source:e};
  }).filter(row => !query || [row.text,row.why,row.title,row.ws,row.source.task,row.source.kind,JSON.stringify(row.source.data)]
    .some(v => String(v ?? '').toLowerCase().includes(query)))
    .sort((a,b) => (b.ts_ms-a.ts_ms) || (b.source.seq-a.source.seq));
  const rows = [];
  let day = null, group = null;
  for (const row of events) {
    const key = dayKey(row.ts_ms);
    if (key !== day) { day = key; group = null; rows.push({key:`jday:${key}:${row.key}`, kind:'day', label:dayLabel(row.ts_ms, now)}); }
    const scope = row.ws || '';
    if (scope !== group) { group = scope; rows.push({key:`jgroup:${row.key}`, kind:'group', title:row.title, ws:row.ws}); }
    rows.push(row);
  }
  return rows;
}
export const journalFields = row => [
  ...(row.why ? [['Why', row.why]] : []),
  ['When', `${dayKey(row.ts_ms)} ${clock(row.ts_ms)}`],
  ['Event', `${row.source.kind} · ${row.source.actor} · #${row.source.seq}`],
  ...(row.source.task ? [['Task', row.source.task]] : []),
  ...Object.entries(row.source.data || {}).filter(([k]) => !['what','why'].includes(k))
    .slice(0, 12).map(([k, v]) => [k, oneLine(typeof v === 'string' ? v : JSON.stringify(v)).slice(0, 300)]),
];
export function railModel({status, log, conversation, board, error = '', now = Date.now(), expandedDays = new Set()}) {
  const entries=log?.entries || [], attention=needsYou(board,entries), questions=attention.filter(item => item.kind === 'question');
  const rows=groupDays(railRows(log,conversation,board,now),now,expandedDays);
  const available = !!status?.coordinator?.terminal && status.enabled && status.state === 'running';
  const working = available && conversation?.coordinator?.status === 'working';
  const canSend = available && !error && ['idle','done'].includes(conversation?.coordinator?.status);
  const since = conversation?.coordinator?.working_since_ms;
  let head = !status ? 'Reading coordinator…' : !status.enabled ? 'Coordinator is off' : !available ? 'Coordinator is not running' : working ? 'Coordinator is working' : 'Coordinator idle · listening';
  let step = working ? conversation?.coordinator?.step?.label || 'Working on the current turn' : questions.length ? `${questions.length} ${questions.length === 1 ? 'question is' : 'questions are'} waiting for your answer` : 'Ready when you are';
  if (error) { head = 'Coordinator unavailable'; step = error; }
  else if (!available && status) step = 'Start or repair the coordinator to continue.';
  else if (['blocked','error'].includes(conversation?.coordinator?.status)) { head = 'Coordinator needs attention'; step = 'Open the raw coordinator to continue.'; }
  else if (conversation?.source?.state && conversation.source.state !== 'ready') step = conversation.source.detail || 'Conversation is not available yet. The raw coordinator stays available.';
  return {rows,questions,needsYou:attention,available:available && !error,canSend,working,head,step,
    elapsed:working && Number.isFinite(since) ? elapsed(now - since) : working ? '…' : 'now',
    active:status?.counts?.open_workstreams || 0, status:!status ? 'Loading' : available ? 'running' : status.enabled ? 'not running' : 'off',
    model:conversation?.coordinator?.model || '',source:conversation?.source, sync:status?.generated_at_ms ? clock(status.generated_at_ms) : '—'};
}
