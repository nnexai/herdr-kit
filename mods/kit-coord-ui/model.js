// The Mission Log consumes coordinator CLI JSON only. No journal or session I/O here.
export function decode(text, kind) {
  const value = JSON.parse(text);
  if (!value || value.schema_version !== 1) throw new Error(`Unsupported coordinator ${kind} format`);
  if (kind === 'log' && !Array.isArray(value.entries)) throw new Error('Invalid coordinator journal');
  if (kind === 'conversation' && !Array.isArray(value.messages)) throw new Error('Invalid coordinator conversation');
  if (kind === 'board' && !Array.isArray(value.workstreams)) throw new Error('Invalid coordinator board');
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
  return row.kind === 'question' && !row.answer && row.options.includes(option)
    ? {text:option, question:{id:row.questionId, time:clock(row.ts_ms)}} : null;
}
export function sayArgs(value, question) {
  if (!value.trim()) return null;
  return ['coord', 'say', '--text', value, ...(question?.id ? ['--question', question.id] : [])];
}
export function railRows(log, conversation, board) {
  const entries = log?.entries || [], rows = [];
  const answers = new Map(entries.filter(e => e.kind === 'question.answered').map(e => [e.data.qid, e.data.text]));
  const workstreams = new Map((board?.workstreams || []).map(ws => [ws.id, ws]));
  const tasks = new Map((board?.workstreams || []).flatMap(ws => (ws.tasks || []).map(task => [task.id, task])));
  for (const message of conversation?.messages || []) {
    if (!['user','assistant'].includes(message.role) || !text(message.text).trim()) continue;
    rows.push({key:`message:${message.id}`, kind:message.role === 'user' ? 'person' : 'reply',
      label:message.role === 'user' ? 'You' : 'Coordinator', ts_ms:message.ts_ms, text:message.text, links:[]});
  }
  for (const entry of entries) {
    const d = entry.data || {}, ws = workstreams.get(entry.ws), task = tasks.get(entry.task);
    const row = {key:`journal:${entry.seq}`, seq:entry.seq, ts_ms:entry.ts_ms, scope:entry.task || entry.ws || '', links:linksFor(task,ws)};
    if (entry.kind === 'task.placed') Object.assign(row,{kind:'placed',label:'Task placed',text:d.title || task?.title || 'Task placed',detail:d.reason,meta:[d.role,d.model].filter(Boolean)});
    else if (entry.kind === 'task.reported') Object.assign(row,{kind:d.outcome === 'question' ? 'question' : 'report',label:d.outcome === 'question' ? 'Delegate question' : 'Delegate report',text:text(d.summary),detail:task?.title,options:[],outcome:d.outcome});
    else if (entry.kind === 'task.progress') Object.assign(row,{kind:'progress',label:'Progress',text:text(d.summary)});
    else if (entry.kind === 'decision') Object.assign(row,{kind:'decision',label:'Decision noted',text:text(d.what),detail:text(d.why)});
    else if (entry.kind === 'instruction.set' && d.text) Object.assign(row,{kind:'decision',label:'Instruction',text:text(d.text)});
    else if (entry.kind === 'question.asked') Object.assign(row,{kind:'question',label:'Question',text:text(d.text),questionId:d.qid,options:(d.options || []).filter(option => typeof option === 'string'),answer:answers.get(d.qid)});
    else if (entry.kind === 'coord.session_started') Object.assign(row,{kind:'neutral',label:'Session',text:'Coordinator session started',detail:d.reason,links:[]});
    else if (entry.kind === 'coord.session_ended') Object.assign(row,{kind:'neutral',label:'Session',text:'Coordinator session ended',detail:d.reason,links:[]});
    else continue;
    rows.push(row);
  }
  // Pending questions outlive the bounded journal page. Board carries their real IDs.
  for (const item of board?.attention || []) {
    if (item.kind !== 'question' || !item.qid || rows.some(row => row.questionId === item.qid)) continue;
    const ws = workstreams.get(item.ws), task = tasks.get(item.task);
    rows.push({key:`journal:${item.seq}`,seq:item.seq,ts_ms:item.since_ms,kind:'question',label:'Question',
      text:item.text,questionId:item.qid,options:item.options || [],scope:item.task || item.ws || '',links:linksFor(task,ws)});
  }
  return rows.sort((a,b) => a.ts_ms - b.ts_ms || (a.seq || 0) - (b.seq || 0) || a.key.localeCompare(b.key));
}
export function railModel({status, log, conversation, board, error = '', now = Date.now()}) {
  const rows = railRows(log,conversation,board);
  const available = !!status?.coordinator?.terminal && status.enabled && status.state === 'running';
  const working = available && conversation?.coordinator?.status === 'working';
  const canSend = available && !error && ['idle','done'].includes(conversation?.coordinator?.status);
  const since = conversation?.coordinator?.working_since_ms;
  const questions = rows.filter(row => row.kind === 'question' && row.questionId && !row.answer);
  let head = !status ? 'Reading coordinator…' : !status.enabled ? 'Coordinator is off' : !available ? 'Coordinator is not running' : working ? 'Coordinator is working' : 'Coordinator idle · listening';
  let step = working ? conversation?.coordinator?.step?.label || 'Working on the current turn' : questions.length ? `${questions.length} ${questions.length === 1 ? 'question is' : 'questions are'} waiting for your answer` : 'Ready when you are';
  if (error) { head = 'Coordinator unavailable'; step = error; }
  else if (!available && status) step = 'Open Board to start or repair the coordinator.';
  else if (['blocked','error'].includes(conversation?.coordinator?.status)) { head = 'Coordinator needs attention'; step = 'Open the raw coordinator to continue.'; }
  else if (conversation?.source?.state && conversation.source.state !== 'ready') step = conversation.source.detail || 'Conversation is not available yet. The raw coordinator stays available.';
  return {rows,questions,available:available && !error,canSend,working,head,step,
    elapsed:working && Number.isFinite(since) ? elapsed(now - since) : working ? '…' : 'now',
    active:status?.counts?.open_workstreams || 0, status:!status ? 'Loading' : available ? 'running' : status.enabled ? 'not running' : 'off',
    model:conversation?.coordinator?.model || '',source:conversation?.source, sync:status?.generated_at_ms ? clock(status.generated_at_ms) : '—'};
}
