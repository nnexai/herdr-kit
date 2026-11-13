// Pure protocol decoders and presentation models for the coordinator board.
const malformed = () => { throw new Error('malformed'); };
const obj = x => x && typeof x === 'object' && !Array.isArray(x) ? x : malformed();
const str = x => typeof x === 'string' ? x.slice(0, 20000) : malformed();
const num = x => Number.isSafeInteger(x) && x >= 0 ? x : malformed();
const bool = x => typeof x === 'boolean' ? x : malformed();
const nullable = fn => x => x === null ? null : fn(x);
const list = (x, max, fn) => Array.isArray(x) && x.length <= max ? x.map(fn) : malformed();
const fields = (x, spec) => { obj(x); const out = {}; for (const [key, fn] of Object.entries(spec)) out[key] = fn(x[key]); return out; };
const ns = nullable(str), nn = nullable(num);
const parse = text => { if (typeof text !== 'string') return malformed(); let bytes=0; for (const ch of text) { const n=ch.codePointAt(0); bytes+=n<0x80?1:n<0x800?2:n<0x10000?3:4; if(bytes>1048576) return malformed(); } try { return obj(JSON.parse(text)); } catch (_) { return malformed(); } };
const envelope = x => { if (x.schema_version !== 1) malformed(); return {schema_version:1, generated_at_ms:num(x.generated_at_ms)}; };
const degraded = x => fields(x,{code:str,since_ms:nn,detail:ns});
const watcher = x => fields(x,{running:bool,terminal:ns,heartbeat_ms:nn,release:ns});
const coordinator = x => fields(x,{terminal:ns,status:ns,space_id:ns,tab_id:ns,session_started_ms:nn});
const counts = x => fields(x,{running:num,needs_you:num,retained:num,open_workstreams:num});
export function decodeStatus(text) { const x=parse(text); return {...envelope(x), ...fields(x,{release:str,state:str,enabled:bool,autostart:bool,degraded:x=>list(x,200,degraded),watcher,coordinator,counts,journal_dir:str,journal_seq:num})}; }
const check = x => fields(x,{name:str,ok:bool,detail:ns});
const retained = x => fields(x,{reason_code:str,reason:str,since_ms:num,checked_ms:nn,auto_clears:bool,checks:x=>list(x,500,check)});
const task = x => fields(x,{id:str,number:num,title:str,state:str,report:ns,start_blocked:bool,stalled:bool,state_since_ms:num,placed_ms:num,started_ms:nn,last_event_ms:num,rung:num,placement_reason:str,rejected_rungs:x=>list(x,500,x=>fields(x,{rung:num,reason:str})),role:str,harness:ns,model:ns,summary:ns,terminal:ns,space_id:ns,tab_id:ns,brief_path:ns,result_path:ns,nudges:num});
const workstream = x => fields(x,{id:str,title:str,state:str,repo:nullable(x=>fields(x,{path:str,name:str})),delivery_target:ns,summary:ns,opened_ms:num,closed_ms:nn,last_event_ms:num,space:nullable(x=>fields(x,{id:str,name:str,owned:bool})),retained:nullable(retained),tasks:x=>list(x,500,task)});
const attention = x => fields(x,{id:str,kind:str,ws:ns,task:ns,since_ms:num,text:str,options:nullable(x=>list(x,200,str)),seq:nn});
export function decodeBoard(text) { const x=parse(text); return {...envelope(x),...fields(x,{journal_seq:num,workstreams:x=>list(x,200,workstream),attention:x=>list(x,200,attention)})}; }
// Journal data is intentionally extensible for unknown event kinds and details.
function dataValue(x, depth=0) { if (depth>20) malformed(); if (x===null || typeof x==='boolean') return x; if (typeof x==='string') return str(x); if (typeof x==='number' && Number.isFinite(x)) return x; if (Array.isArray(x)) return list(x,500,v=>dataValue(v,depth+1)); obj(x); const keys=Object.keys(x); if(keys.length>64) malformed(); return Object.fromEntries(keys.map(k=>[str(k),dataValue(x[k],depth+1)])); }
const entry = x => fields(x,{v:num,seq:num,ts_ms:num,actor:str,kind:str,ws:ns,task:ns,data:x=>dataValue(obj(x))});
export function decodeLog(text) { const x=parse(text); return {...envelope(x),...fields(x,{entries:x=>list(x,500,entry),has_more:bool,newest_seq:nn,oldest_seq:nn})}; }
export const clip = (text,max) => { const s=String(text ?? ''); return s.length>max ? s.slice(0,Math.max(0,max-1))+'…' : s; };
export const oneLine = text => String(text ?? '').replace(/\s+/g,' ').trim();
const formatDateTime = ms => { const d=new Date(ms); return `${d.getFullYear()}-${pad(d.getMonth()+1)}-${pad(d.getDate())} ${formatClock(ms)}`; };
export const estimateWidth = text => Math.ceil(String(text).length*6.2);
export function formatAge(ms) { let s=Math.max(0,Math.floor(ms/1000)); if(s<60) return `${s}s`; let m=Math.floor(s/60); if(m<60) return `${m}m${s%60 ? ` ${s%60}s` : ''}`; let h=Math.floor(m/60); if(h<24) return `${h}h${m%60 ? ` ${m%60}m` : ''}`; return `${Math.floor(h/24)}d${h%24 ? ` ${h%24}h` : ''}`; }
const pad = n => String(n).padStart(2,'0');
export function formatClock(ms) { const d=new Date(ms); return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`; }
export function formatDay(ms,now) { const d=new Date(ms), n=new Date(now), day=new Date(d.getFullYear(),d.getMonth(),d.getDate()).getTime(), today=new Date(n.getFullYear(),n.getMonth(),n.getDate()).getTime(); if(day===today) return 'TODAY'; const y=new Date(n.getFullYear(),n.getMonth(),n.getDate()-1).getTime(); return day===y ? 'YESTERDAY' : `${['Sun','Mon','Tue','Wed','Thu','Fri','Sat'][d.getDay()]} ${['Jan','Feb','Mar','Apr','May','Jun','Jul','Aug','Sep','Oct','Nov','Dec'][d.getMonth()]} ${d.getDate()}`; }
export const rungLabel = n => ({1:'same session',2:'same space',3:'other space',4:'new worktree',5:'new project'})[n] ?? '';
export const reasonText = code => ({watcher_heartbeat_stale:'watcher stalled',watcher_missing:'watcher stopped',coordinator_terminal_missing:'coordinator missing',hc_unreachable:'hc unreachable'})[code] ?? String(code ?? '').replace(/_/g,' ');
export function degradedSentence(status) { return (status?.degraded ?? []).map(x=>x.detail || reasonText(x.code)).join('; '); }
export const shouldAutostart = (status,tried) => !!status && !tried && status.enabled && status.autostart && status.state !== 'running';
export function statusbarModel(status,ctx={}) {
 const now=ctx.now ?? 0, age=now-(ctx.lastOkMs ?? status?.generated_at_ms ?? now), stale=!!status && !!ctx.failed && age>90000;
 let state=ctx.starting?'starting':(!status ? (ctx.failed?'unavailable':'loading') : ctx.failed && (age>120000 || (ctx.failures ?? ctx.failureCount ?? 0)>=3) ? 'unavailable' : !status.enabled?'not_running':status.state);
 const c=status?.counts ?? {running:0,needs_you:0,retained:0};
 const segments=[['running',c.running,'running','muted'],['needs_you',c.needs_you,'needs you','blocked'],['retained',c.retained,'retained','muted']].filter(x=>x[1]>0).map(([key,n,label,role])=>({key,text:`${n} ${label}`,compact:String(n),role}));
 let text='',compactText='',tooltip='',click='reread';
 if(state==='running') { text=segments.map(s=>s.text).join(' · ') || 'No tasks'; compactText=segments.map(s=>`● ${s.compact}`).join(' ') || 'No tasks'; const clauses=[c.running?`${c.running} tasks running`:null,c.needs_you?`${c.needs_you} needs you`:null,c.retained?`${c.retained} workstreams retained`:null].filter(Boolean); tooltip=`Coordinator running. ${clauses.join(', ') || 'No tasks'}. Click to open the board.`; click='board'; }
 else if(state==='not_running') { text='Coordinator off'; compactText='Off'; tooltip=status?.enabled?'Coordinator is not running. Click to start it. Right-click opens the board.':'Coordinator is off. Click to start it: this creates a "coordinator" space and starts the coordinator agent. Right-click opens the board.'; click='up'; }
 else if(state==='degraded') { const reasons=status.degraded ?? []; text=`Degraded${reasons.length?` · ${reasonText(reasons[0].code)}${reasons.length>1?` +${reasons.length-1}`:''}`:''}${c.needs_you?` · ${c.needs_you} needs you`:''}`; compactText='Degraded'; tooltip=`Coordinator degraded: ${degradedSentence(status)}. Click to repair (coord up). Right-click opens the board.`; click='up'; }
 else if(state==='starting') { text='Starting…'; compactText='…'; tooltip='Starting the coordinator…'; click='none'; }
 else if(state==='loading') { text='Coordinator …'; compactText='…'; tooltip='Reading coordinator status…'; }
 else { state='unavailable'; text='Coordinator unavailable'; compactText='n/a'; tooltip=`Coordinator status unavailable: ${clip(oneLine((ctx.error ?? ctx.firstLine ?? '').split('\n')[0]),120)}. Click to retry.`; }
 if(stale) { text+=' ~'; compactText+=' ~'; tooltip+=` Last updated ${formatClock(status.generated_at_ms).slice(0,5)}.`; }
 const inner=(ctx.width ?? 280)-53, tier=estimateWidth(text)<=inner?'full':estimateWidth(compactText)<=inner?'dots':'icon';
 return {state,tier,icon:({running:'coord-running.svg',not_running:'coord-off.svg',degraded:'coord-degraded.svg',starting:'coord-starting.svg'})[state] ?? 'coord-off.svg',iconRole:({running:'accent',degraded:'working'})[state] ?? 'dim',segments,text,compactText,tooltip,label:tooltip,click,stale};
}
const action = (id,label,enabled=true,primary=false) => ({id,label,primary,enabled});
export function rowActions(row,status,liveAgents) { const s=row.source ?? row; const kind=row.kind; const actions=[]; if(kind==='attention' && ['question','task_question','coordinator_blocked'].includes(s.kind)) { if(status?.coordinator?.terminal) actions.push(action('reply','Reply in coordinator')); } else if(kind==='task' || kind==='attention') { const t=kind==='task'?s:row.task; if(t && t.state!=='dormant' && t.state!=='closed' && t.terminal) actions.push(action('show','Show'),action('peek','Peek')); else if(t?.space_id) actions.push(action('show-space','Show space')); } else if((kind==='workstream'||kind==='retained') && s.space?.id) actions.push(action('show-space','Show space')); if(['task','attention','workstream','retained'].includes(kind)) actions.push(action('log','Log')); return actions; }
export function actionCommand(actionOrId,row,status) { const id=typeof actionOrId==='string'?actionOrId:actionOrId?.id, s=row?.source ?? row ?? {}, task=row?.kind==='attention'?row.task:s; if(id==='up'||id==='start'||id==='repair') return {kind:'up'}; if(id==='toggle'||id==='details'||id==='closed') return {kind:'toggle'}; if(id==='copy') return {kind:'copy',text:actionOrId.text ?? ''}; if(id==='log') return {kind:'log',scope:row.kind==='task'?{task:s.id}:row.kind==='attention'?(s.task?{task:s.task}:s.ws?{ws:s.ws}:{}):{ws:s.id}}; if(id==='reply' && status?.coordinator?.terminal) return {kind:'run',name:'agent.focus',args:{terminal:status.coordinator.terminal}}; if((id==='show'||id==='peek') && task?.terminal) return {kind:'run',name:id==='show'?'agent.focus':'terminal.peek',args:{terminal:task.terminal}}; if(id==='show-space') { const space=task?.space_id ?? s.space?.id; if(space) return {kind:'run',name:'space.select',args:{id:space},...(task?.tab_id?{then:{name:'tab.select',args:{id:task.tab_id}}}:{})}; } if(id==='show-tab' && task?.tab_id) return {kind:'run',name:'tab.select',args:{id:task.tab_id}}; if(id==='show-coordinator' && status?.coordinator?.terminal) return {kind:'run',name:'agent.focus',args:{terminal:status.coordinator.terminal}}; return null; }
const detail = (label,value,copy=false) => ({label,value:String(value),...(copy?{copy:true}:{})});
function taskState(t) { if(t.state==='placed') return t.start_blocked?['stuck','blocked']:['starting','idle']; if(t.state==='running') return t.start_blocked?['stuck','blocked']:t.stalled?['stalled','blocked']:['running','working']; if(t.state==='reported') return ({done:['done','done'],blocked:['blocked','blocked'],question:['asking','blocked']})[t.report] ?? [t.report ?? 'reported','unknown']; if(t.state==='dormant'||t.state==='closed') return [t.state,'dim']; return [t.state,'unknown']; }
export function taskRow(t,ctx={}) { const [word,role]=taskState(t), placement=`${t.id} · rung ${t.rung} ${rungLabel(t.rung)} — ${oneLine(t.placement_reason)}`, rejected=t.rejected_rungs.map(r=>`rung ${r.rung} ${rungLabel(r.rung)} — ${oneLine(r.reason)}`); const details=[detail('Task',t.id),...(t.brief_path?[detail('Brief',t.brief_path,true)]:[]),...(t.result_path?[detail('Result',t.result_path,true)]:[]),...(t.summary?[detail('Summary',clip(t.summary,600))]:[]),...(t.model?[detail('Model',t.model)]:[]),...(t.harness?[detail('Harness',t.harness)]:[]),detail('Placed',formatDateTime(t.placed_ms)),...(t.started_ms!==null?[detail('Started',formatDateTime(t.started_ms))]:[]),detail('Nudges',t.nudges),...rejected.map(v=>detail('Rejected rungs',v))]; const row={key:t.id,kind:'task',depth:1,selectable:true,title:oneLine(t.title),meta:clip(placement,240),state:{word,role},age:formatAge((ctx.now ?? 0)-t.state_since_ms),details,tooltip:[placement,...rejected].join('\n'),source:t,role:t.role,model:t.model,harness:t.harness}; if(!t.terminal&&(t.state==='dormant'||t.state==='closed')) row.tooltip+='\nAgent ended; session is recorded and can be resumed by the coordinator.'; row.actions=rowActions(row,ctx.status,ctx.liveAgents); return row; }
export function attentionRow(item,ctx={}) { const task=ctx.task ?? null, origins={question:'asked by coordinator',task_question:'asked by delegate',task_blocked:'delegate blocked',start_blocked:`did not start after ${formatAge((ctx.now ?? 0)-item.since_ms)} (start-up dialog?)`,stalled:`no report ${formatAge((ctx.now ?? 0)-item.since_ms)} after nudge`,coordinator_blocked:'coordinator is blocked'}; const known=Object.prototype.hasOwnProperty.call(origins,item.kind); const row={key:item.id,kind:'attention',depth:0,selectable:true,title:oneLine(item.text),meta:[ctx.workstream?.title,item.task,known?origins[item.kind]:item.kind].filter(Boolean).join(' · '),state:{word:known?(item.kind==='question'||item.kind==='task_question'?'?':'!'):item.kind,role:known?'blocked':'dim'},age:formatAge((ctx.now ?? 0)-item.since_ms),details:[detail('Text',clip(item.text,600)),...(item.options ?? []).map(x=>detail('Option',x)),...(item.ws?[detail('Workstream',item.ws)]:[]),...(item.task?[detail('Task',item.task)]:[]),detail('Since',formatDateTime(item.since_ms))],tooltip:item.text,source:item,task}; row.actions=rowActions(row,ctx.status,ctx.liveAgents); return row; }
export function workstreamRow(ws,ctx={}) { const row={key:ws.id,kind:'workstream',depth:0,selectable:true,title:ws.title,meta:[ws.id,ws.repo?.name && `${ws.repo.name}${ws.delivery_target?` → ${ws.delivery_target}`:''}`,`${ws.tasks.filter(t=>t.state!=='closed').length} tasks`].filter(Boolean).join(' · '),state:{word:ws.state==='closed'?'closing':ws.state,role:'muted'},age:formatAge((ctx.now ?? 0)-ws.last_event_ms),details:[],tooltip:ws.summary ?? ws.title,source:ws,collapsed:!!ctx.collapsed}; row.actions=rowActions(row,ctx.status,ctx.liveAgents); return row; }
export function retainedRow(ws,ctx={}) { const r=ws.retained, row={key:ws.id,kind:'retained',depth:0,selectable:true,title:ws.title,meta:`${r.reason}${r.auto_clears?' — cleared automatically once the blocking condition is gone.':''}`,state:{word:'retained',role:'working'},age:formatAge((ctx.now ?? 0)-r.since_ms),details:r.checks.map(c=>detail(`${c.ok?'✓':'✕'} ${c.name}`,c.detail ?? '')),tooltip:r.reason,source:ws,checked:r.checked_ms===null?null:formatAge((ctx.now ?? 0)-r.checked_ms)}; row.actions=rowActions(row,ctx.status,ctx.liveAgents); return row; }
export function boardModel(board,status,ui={}) { const now=ui.now ?? 0, collapsed=ui.collapsed ?? new Set(), showClosed=ui.showClosed ?? new Set(), ctx={now,status,liveAgents:ui.liveAgents}; const ws=board?.workstreams ?? [], att=[...(board?.attention ?? [])].sort((a,b)=>a.since_ms-b.since_ms), attentionWs=new Set(att.map(a=>a.ws)); const sections=[]; if(att.length) sections.push({id:'attention',title:'Needs you',count:att.length,rows:att.map(a=>{const w=ws.find(w=>w.id===a.ws); return attentionRow(a,{...ctx,workstream:w,task:w?.tasks.find(t=>t.id===a.task)});})}); const active=ws.filter(w=>!w.retained).sort((a,b)=>Number(attentionWs.has(b.id))-Number(attentionWs.has(a.id))||b.last_event_ms-a.last_event_ms); const rows=[]; for(const w of active) { const defaultOpen=w.tasks.some(t=>t.state!=='closed')||attentionWs.has(w.id), isCollapsed=collapsed.has(w.id)||(!defaultOpen&&!collapsed.has(`open:${w.id}`)); rows.push(workstreamRow(w,{...ctx,collapsed:isCollapsed})); if(isCollapsed) continue; const tasks=w.tasks.filter(t=>t.state!=='closed').sort((a,b)=>{const rank=t=>({stuck:0,stalled:0,blocked:0,asking:0,running:1,starting:2,done:3,dormant:4})[taskState(t)[0]] ?? 5; return rank(a)-rank(b)||a.number-b.number;}); rows.push(...tasks.map(t=>taskRow(t,ctx))); const closed=w.tasks.filter(t=>t.state==='closed').sort((a,b)=>a.number-b.number); if(closed.length) { rows.push({key:`${w.id}:closed`,kind:'closed',depth:1,selectable:true,title:`${closed.length} closed ${closed.length===1?'task':'tasks'}`,source:w,actions:[action('toggle','Toggle')]}); if(showClosed.has(w.id)) rows.push(...closed.map(t=>taskRow(t,ctx))); } } if(active.length) sections.push({id:'workstreams',title:'Workstreams',count:active.length,rows}); const retained=ws.filter(w=>w.retained).sort((a,b)=>b.retained.since_ms-a.retained.since_ms); if(retained.length) sections.push({id:'retained',title:'Retained',count:retained.length,rows:retained.map(w=>retainedRow(w,ctx))}); const flat=sections.flatMap(s=>s.rows); const health=[status?.watcher?.running?'Watcher alive':'Watcher not running',status?.watcher?.heartbeat_ms!=null?`heartbeat ${formatAge(now-status.watcher.heartbeat_ms)} ago`:null,status?.watcher?.release?`release ${status.watcher.release.slice(0,7)}`:null,status?.coordinator?.status?`coordinator ${status.coordinator.status}`:null,status?.generated_at_ms!=null?`updated ${formatClock(status.generated_at_ms)}`:null].filter(Boolean).join(' · '); return {banner:status?.state==='degraded'?`Degraded: ${degradedSentence(status)}.${status.degraded.some(d=>d.code.startsWith('watcher_'))?' Tasks keep running; nudges and reconcile are paused.':''}`:null,header:{state:status?.state ?? 'loading',terminal:status?.coordinator?.terminal ?? null},health,sections,rows:flat,empty:flat.length?null:{kind:board?'empty':'loading'}}; }
export function moveSelection(rows,selectedKey,key,page=10) { const selectable=rows.map((r,i)=>r.selectable?i:-1).filter(i=>i>=0); if(!selectable.length) return null; let i=selectable.findIndex(n=>rows[n].key===selectedKey); if(i<0) i=0; if(key==='home') i=0; else if(key==='end') i=selectable.length-1; else if(key==='up') i--; else if(key==='down') i++; else if(key==='pageup') i-=page; else if(key==='pagedown') i+=page; return rows[selectable[Math.max(0,Math.min(selectable.length-1,i))]].key; }
export function reconcileSelection(rows,prevKey,prevIndex=0) { if(rows.some(r=>r.selectable&&r.key===prevKey)) return prevKey; let best=null, distance=Infinity; rows.forEach((r,i)=>{if(r.selectable && Math.abs(i-prevIndex)<distance){best=r.key;distance=Math.abs(i-prevIndex);}}); return best; }

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
export function timelineModel(log,ui={}) {
 const entries=[...(log?.entries ?? [])].sort((a,b)=>b.seq-a.seq).slice(0,2000), noisy=e=>['inbox.drained','res.intent','watcher.started','watcher.stopped','nudge.sent'].includes(e.kind)||(e.kind==='res.release_checked'&&e.data.ok===true), hiddenCount=ui.detail==='all'?0:entries.filter(noisy).length, filter=oneLine(ui.text ?? '').toLowerCase();
 const visible=entries.filter(e=>(ui.detail==='all'||!noisy(e))&&(!filter||[e.ws,e.task,e.kind,e.actor,summarize(e).text].some(x=>String(x ?? '').toLowerCase().includes(filter))));
 const rows=[]; let lastDay=null; for(const e of visible) { const day=formatDay(e.ts_ms,ui.now ?? 0); if(day!==lastDay) {rows.push({key:`day:${day}:${e.seq}`,kind:'day',selectable:false,title:day});lastDay=day;} const summary=summarize(e); rows.push({key:String(e.seq),kind:'entry',selectable:true,title:summary.text,...summary,source:e,time:formatClock(e.ts_ms),expanded:(ui.expanded ?? new Set()).has(e.seq)}); }
 if(log?.gap) rows.push({key:'more',kind:'more',selectable:true,title:'Load missing older entries (200)'});
 else if(log?.has_more) rows.push({key:'more',kind:'more',selectable:entries.length<2000,title:entries.length>=2000?'Showing the newest 2000 entries. Narrow the scope to see older ones.':'Load older entries (200)'});
 const scope=ui.scope ?? {}, scopeId=scope.task ?? scope.ws ?? null, loadedNoMatch=entries.length>0 && ((filter && (log?.has_more || log?.gap)) || (!filter && hiddenCount>0));
 return {rows,hiddenCount,scopeChip:scopeId?`Scope: ${scopeId}`:null,empty:visible.length?null:{kind:filter?'filter':loadedNoMatch?'loaded':scopeId?'scope':log?'empty':'loading',text:loadedNoMatch?'Nothing on the loaded pages matches the current filter.':filter?`No entries match “${ui.text}”.`:scopeId?`No entries for ${scopeId}.`:'The journal is empty.'},banner:ui.banner ?? null,newerPill:ui.newerCount?`${ui.newerCount} new entries ↑`:null};
}
// Keep native element construction bounded while retaining the complete loaded
// timeline for filtering and keyboard navigation.
export function timelineWindow(rows, selectedKey, start = 0, size = 120) {
 const maxStart=Math.max(0,rows.length-size);
 let first=Math.max(0,Math.min(maxStart,start));
 const selected=rows.findIndex(row=>row.key===selectedKey);
 if(selected>=0 && selected<first) first=selected;
 else if(selected>=first+size) first=Math.min(maxStart,selected-size+1);
 return {start:first,end:Math.min(rows.length,first+size),newer:first>0,older:first+size<rows.length};
}

// Presentation states are kept with the row models so cached and first-read
// failures have the same wording in the board and timeline.
const refreshBanner = (snapshot,error) =>
 `Showing data from ${formatClock(snapshot.generated_at_ms).slice(0,5)} — refresh failed: ${clip(oneLine(String(error ?? '').split('\n')[0]),160)}.`;
export function boardViewModel(board,status,ui={}) {
 const vm=boardModel(board,status,ui), failed=!!ui.failed;
 if(status?.degraded?.some(d=>d.code==='watcher_heartbeat_stale')) vm.health=vm.health.replace(/^Watcher (?:alive|not running)/,'Watcher stalled');
 vm.header.state=ui.starting?'starting':!status?.enabled && status?'not_running':vm.header.state;
 vm.bannerAction=vm.banner?'Repair':null;
 if(board && failed) { vm.banner=refreshBanner(board,ui.error); vm.bannerAction='Retry'; }
 else if(ui.statusFailed) { vm.banner=status?refreshBanner(status,ui.statusError):`Coordinator status unavailable: ${clip(oneLine(String(ui.statusError ?? '').split('\n')[0]),160)}.`; vm.bannerAction='Retry'; }
 else if(board && vm.rows.length && status && !status.enabled) { vm.banner='Coordinator is not running.'; vm.bannerAction='Start coordinator'; }
 if(!board) vm.empty=failed
  ? {kind:'error',text:"Can't read the board.",detail:clip(oneLine(String(ui.error ?? '').split('\n')[0]),160),action:'Retry'}
  : {kind:'loading',text:'Reading board…'};
 else if(!vm.rows.length) vm.empty=status && !status.enabled
  ? {kind:'off',text:'The coordinator is off.',detail:'Starting it creates a "coordinator" space and starts the coordinator agent.',action:'Start coordinator'}
  : {kind:'empty',text:'No work in flight.',detail:'Tell the coordinator what you want done; it is in the "coordinator" space.',action:status?.coordinator?.terminal?'Show coordinator':null};
 return vm;
}
export function timelineViewModel(log,ui={}) {
 const vm=timelineModel(log,ui);
 if(log && ui.failed) { vm.banner=refreshBanner(log,ui.error); vm.bannerAction='Retry'; }
 else if(ui.statusFailed) { vm.banner=ui.status?refreshBanner(ui.status,ui.statusError):`Coordinator status unavailable: ${clip(oneLine(String(ui.statusError ?? '').split('\n')[0]),160)}.`; vm.bannerAction='Retry'; }
 if(!log) vm.empty=ui.failed
  ? {kind:'error',text:"Can't read the journal.",detail:clip(oneLine(String(ui.error ?? '').split('\n')[0]),160),action:'Retry'}
  : {kind:'loading',text:'Reading journal…'};
 else if(vm.empty) {
  if(vm.empty.kind==='filter') vm.empty.action='Clear filter';
  else if(vm.empty.kind==='scope') vm.empty.action='Clear scope';
  else if(vm.empty.kind==='empty') vm.empty.detail='Nothing has happened yet.';
 }
 return vm;
}
export function mergeLog(prev,next,mode='replace') { const a=Array.isArray(prev)?prev:prev?.entries ?? [], b=Array.isArray(next)?next:next?.entries ?? []; const map=new Map(); for(const e of mode==='replace'?b:mode==='prepend'?[...a,...b]:[...b,...a]) map.set(e.seq,e); return [...map.values()].sort((a,b)=>b.seq-a.seq).slice(0,2000); }
// A refresh reads newest-first pages, bounded to one display window.
// `beforeSeq` continues towards the old cursor while `afterSeq` keeps the
// request inside the new range, including for scoped (non-contiguous) logs.
export function mergeRefreshPage(state,page) {
 const entries=mergeLog(state.entries ?? [],page.entries,'append');
 const more=page.has_more && page.entries.length>0;
 return {...state,entries,latest:state.latest ?? page,
  beforeSeq:more && entries.length<2000 ? page.oldest_seq : null,
  gap:!!state.gap || (more && entries.length>=2000)};
}
export function mergeTimelinePage(prev,next,mode='prepend',refreshGap=false) {
 const gap=mode==='prepend' && (refreshGap || prev.gap || prev.entries.length+next.entries.length>2000);
 const entries=mergeLog(prev.entries,next.entries,mode==='append' && prev.gap?'replace':mode);
 return {...next,entries,gap,newest_seq:entries[0]?.seq ?? null,oldest_seq:entries[entries.length-1]?.seq ?? null,
  has_more:mode==='prepend' ? prev.has_more || gap : next.has_more};
}
export function shouldDeferTimeline(rows,selectedKey,pending=[]) {
 if(pending.length) return true;
 const first=rows.find(row=>row.selectable);
 return !!first && selectedKey != null && selectedKey!==first.key;
}
export function mergePendingEntries(pending,incoming,visible=[]) {
 const shown=new Set((Array.isArray(visible)?visible:visible?.entries ?? []).map(e=>e.seq));
 const fresh=(Array.isArray(incoming)?incoming:incoming?.entries ?? []).filter(e=>!shown.has(e.seq));
 return mergeLog(pending.filter(e=>!shown.has(e.seq)),fresh,'prepend');
}
export function logArgs(scope={},options={}) { const args=['--json','--limit',String(options.limit ?? 200)]; if(scope.task) args.push('--task',scope.task); else if(scope.ws) args.push('--ws',scope.ws); if(options.beforeSeq!=null) args.push('--before-seq',String(options.beforeSeq)); if(options.afterSeq!=null) args.push('--after-seq',String(options.afterSeq)); return args; }
export function nextRefreshDelay(state,now) { if(state.inFlight || state.inflight) return {waitMs:null}; const remaining=750-(now-(state.lastRunMs ?? state.lastMs ?? -Infinity)); return remaining<=0?{runNow:true}:{waitMs:remaining}; }
