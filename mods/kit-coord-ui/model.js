// The Mission Log consumes coordinator CLI JSON only. No journal or session I/O here.
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
const clip = (value, max) => { const s=String(value ?? ''); return s.length>max ? s.slice(0,Math.max(0,max-1))+'…' : s; };
const oneLine = value => String(value ?? '').replace(/\s+/g,' ').trim();
const pad = n => String(n).padStart(2,'0');
const formatClock = ms => { const d=new Date(ms); return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`; };
const formatDateTime = ms => { const d=new Date(ms); return `${d.getFullYear()}-${pad(d.getMonth()+1)}-${pad(d.getDate())} ${formatClock(ms)}`; };
const formatAge = ms => { let s=Math.max(0,Math.floor(ms/1000)); if(s<60) return `${s}s`; let m=Math.floor(s/60); if(m<60) return `${m}m${s%60 ? ` ${s%60}s` : ''}`; let h=Math.floor(m/60); if(h<24) return `${h}h${m%60 ? ` ${m%60}m` : ''}`; return `${Math.floor(h/24)}d${h%24 ? ` ${h%24}h` : ''}`; };
const rungLabel = n => ({1:'same session',2:'same space',3:'other space',4:'new worktree',5:'new project'})[n] ?? '';
const detail = (label,value,copy=false) => ({label,value:String(value),...(copy?{copy:true}:{})});

// Kept local so kit-coord-ui depends on the CLI contract, not another mod.
export function summarize(e) {
 const d=e.data ?? {}, v=k=>typeof d[k]==='string'||typeof d[k]==='number'?clip(oneLine(d[k]),90):'', part=(prefix,k,suffix='')=>v(k)?`${prefix}${v(k)}${suffix}`:'', resource=()=>[v('type'),v('name')].filter(Boolean).join(' ');
 let glyph='·',tone='quiet',text=e.kind;
 switch(e.kind) {
  case 'ws.opened': glyph='◻';tone='neutral';text='Opened workstream'+part(' “','title','”')+(v('repo')||v('delivery_target')?` (${[v('repo'),v('delivery_target')].filter(Boolean).join(' → ')})`:'');break;
  case 'ws.closed': glyph='◼';tone='neutral';text='Closed workstream'+part(' — ','summary');break;
  case 'ws.cleaned': glyph='✓';tone='ok';text='Cleaned up workstream';break;
  case 'ws.retained': glyph='⚠';tone='warning';text='Retained'+part(': ','reason');break;
  case 'task.placed': glyph='▣';tone='neutral';text=['Placed',v('role'),v('rung')?`rung ${v('rung')} ${rungLabel(d.rung)}`.trim():''].filter(Boolean).join(' · ')+part(' — ','reason');break;
  case 'task.started': glyph='●';tone='neutral';text='Started'+part(' in tab “','tab_name','”')+part(' (','terminal',')');break;
  case 'task.reported': glyph=d.outcome==='done'?'✓':d.outcome==='question'?'?':'!';tone=d.outcome==='done'?'ok':'attention';text='Reported'+part(' ','outcome')+part(': ','summary');break;
  case 'task.progress': glyph='●';tone='neutral';text='Progress'+part(': ','summary');break;
  case 'task.closed': glyph='✓';text='Closed task'+part(' — ','reason');break;
  case 'task.sent': glyph='›';tone='neutral';text='Sent follow-up to delegate';break;
  case 'task.note': glyph='✎';tone='neutral';text='Task note'+part(': ','text');break;
  case 'task.dormant': glyph='◌';text='Agent ended; session recorded (dormant)';break;
  case 'task.start_blocked': glyph='!';tone='attention';text='Did not start'+part(' within ','after_secs','s')+' — probably a start-up dialog';break;
  case 'task.stalled': glyph='!';tone='attention';text='Stalled: no report'+(typeof d.silent_secs==='number'?` ${formatAge(d.silent_secs*1000)}`:'')+' after nudge';break;
  case 'res.intent': text='Intent: create'+(resource()?` ${resource()}`:'');break;
  case 'res.created': case 'res.adopted': case 'res.released': glyph=e.kind==='res.released'?'−':'+';tone='neutral';text=({'res.created':'Created','res.adopted':'Adopted','res.released':'Released'})[e.kind]+(resource()?` ${resource()}`:'')+(e.kind==='res.adopted'?' (yours; never removed)':'');break;
  case 'res.release_checked': glyph=d.ok?'✓':'✕';tone=d.ok?'quiet':'warning';text='Release check'+part(' · ','check')+(typeof d.ok==='boolean'?` → ${d.ok?'passed':'failed'}`:'')+part(' (','detail',')');break;
  case 'nudge.sent': glyph='›';text='Nudged'+part(' ','target');break;
  case 'reconcile.finding': glyph='⚠';tone='warning';text='Reconcile'+part(': ','summary');break;
  case 'watcher.started': text='Watcher started'+(v('release')?` (release ${v('release').slice(0,7)})`:'');break;
  case 'watcher.stopped': text='Watcher stopped'+part(' — ','reason');break;
  case 'coord.session_started': glyph='●';tone='neutral';text='Coordinator session started'+part(' (','role',')');break;
  case 'coord.session_ended': glyph='○';tone='neutral';text='Coordinator session ended'+part(' — ','reason');break;
  case 'coord.handoff_requested': glyph='↗';tone='neutral';text='Coordinator handoff requested'+part(' by ','by')+part(': ','note');break;
  case 'coord.sent': glyph='›';tone='neutral';text='Sent to coordinator'+part(': ','text');break;
  case 'inbox.drained': text='Inbox drained'+part(' (','count',')');break;
  case 'decision': glyph='↗';tone=d.tier==='ask'?'attention':'neutral';text='Decision'+part(' (','tier',')')+part(': ','what')+part(' — ','why');break;
  case 'question.asked': glyph='?';tone='attention';text='Asked'+part(': ','text');break;
  case 'question.answered': glyph='✓';tone='ok';text='Answered'+part(': ','text');break;
  case 'scout.result': glyph='⌕';tone='neutral';text='Scout'+part(': ','summary');break;
  case 'instruction.set': glyph='✎';tone='neutral';text='Standing instruction'+part(': ','text');break;
  case 'thread.opened': case 'thread.closed': glyph=e.kind==='thread.opened'?'◻':'◼';tone='neutral';text=(e.kind==='thread.opened'?'Opened':'Closed')+' thread'+part(' “','title','”');break;
  case 'correction': glyph='⟲';tone='warning';text='You corrected'+part(': ','what')+part(' — ','why');break;
  default: text=e.kind+part(' — ',['summary','text','title'].find(k=>typeof d[k]==='string') ?? '');
 }
 const details=[detail('Entry',`seq ${e.seq} · ${formatDateTime(e.ts_ms)} · actor ${e.actor} · v${e.v}`),...Object.entries(d).slice(0,12).map(([k,value])=>detail(k,clip(typeof value==='string'?value:JSON.stringify(value),300),/brief|result|path/i.test(k) && typeof value==='string'))];
 return {glyph,tone,text,scopeId:e.task ?? e.ws ?? '',actorLabel:e.actor,details};
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
    result.push({key:`day:${day}`,kind:'day',day,label:dayLabel(items[0].ts_ms,now),ts_ms:items[0].ts_ms,collapsed,count,countLabel,summary});
    if (!collapsed) result.push(...items);
  }
  return result;
}
export function journalRows(log, board, {filter = '', showSystem = false} = {}) {
  const workstreams=new Map((board?.workstreams || []).map(ws => [ws.id,ws]));
  const query=oneLine(filter).toLowerCase();
  const system=e => /^(watcher\.|inbox\.|nudge\.|res\.|coord\.turn$|task\.turn$|task\.launch_details$|task\.session$)/.test(e.kind);
  return (log?.entries || []).filter(e => showSystem || !system(e)).map(e => {
    const ws=workstreams.get(e.ws), summary=summarize(e);
    return {key:`journal:${e.seq}`,ts_ms:e.ts_ms,ws:e.ws,title:ws?.title || '',...summary,source:e};
  }).filter(row => !query || [row.text,row.title,row.ws,row.source.task,row.source.kind,row.source.actor,JSON.stringify(row.source.data)].some(v => String(v ?? '').toLowerCase().includes(query)))
    .sort((a,b) => (b.ts_ms-a.ts_ms) || (b.source.seq-a.source.seq));
}
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
