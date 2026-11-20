// Pure protocol decoder and presentation model for coordinator status.
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
const coordinator = x => ({...fields(x,{terminal:ns,status:ns,space_id:ns,tab_id:ns,session_started_ms:nn}), ...(x.restart_pending === undefined ? {} : {restart_pending:bool(x.restart_pending)})});
const counts = x => fields(x,{running:num,needs_you:num,retained:num,open_workstreams:num});
export function decodeStatus(text) { const x=parse(text); return {...envelope(x), ...fields(x,{release:str,state:str,enabled:bool,autostart:bool,degraded:x=>list(x,200,degraded),watcher,coordinator,counts,journal_dir:str,journal_seq:num})}; }
export const clip = (text,max) => { const s=String(text ?? ''); return s.length>max ? s.slice(0,Math.max(0,max-1))+'…' : s; };
export const oneLine = text => String(text ?? '').replace(/\s+/g,' ').trim();
export const estimateWidth = text => Math.ceil(String(text).length*6.2);
const pad = n => String(n).padStart(2,'0');
export function formatClock(ms) { const d=new Date(ms); return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`; }
export const reasonText = code => ({watcher_heartbeat_stale:'watcher stalled',watcher_missing:'watcher stopped',coordinator_terminal_missing:'coordinator missing',hc_unreachable:'hc unreachable'})[code] ?? String(code ?? '').replace(/_/g,' ');
export function degradedSentence(status) { return (status?.degraded ?? []).map(x=>x.detail || reasonText(x.code)).join('; '); }
export const shouldAutostart = (status,tried) => !!status && !tried && status.enabled && status.autostart && status.state !== 'running';
export function statusbarModel(status,ctx={}) {
 const now=ctx.now ?? 0, age=now-(ctx.lastOkMs ?? status?.generated_at_ms ?? now), stale=!!status && !!ctx.failed && age>90000;
 let state=ctx.starting?'starting':(!status ? (ctx.failed?'unavailable':'loading') : ctx.failed && (age>120000 || (ctx.failures ?? 0)>=3) ? 'unavailable' : !status.enabled?'not_running':status.state);
 const c=status?.counts ?? {running:0,needs_you:0,retained:0};
 const segments=[['running',c.running,'running','muted'],['needs_you',c.needs_you,'needs you','blocked'],['retained',c.retained,'retained','muted']].filter(x=>x[1]>0).map(([key,n,label,role])=>({key,text:`${n} ${label}`,compact:String(n),role}));
 let text='',compactText='',tooltip='',click='reread';
 if(state==='running') { text=segments.map(s=>s.text).join(' · ') || 'No tasks'; compactText=segments.map(s=>`● ${s.compact}`).join(' ') || 'No tasks'; const clauses=[c.running?`${c.running} tasks running`:null,c.needs_you?`${c.needs_you} needs you`:null,c.retained?`${c.retained} workstreams retained`:null].filter(Boolean); tooltip=`Coordinator running. ${clauses.join(', ') || 'No tasks'}. Click to focus the coordinator.`; click='focus'; }
 else if(state==='not_running') { text='Coordinator off'; compactText='Off'; tooltip=status?.enabled?'Coordinator is not running. Click to start it.':'Coordinator is off. Click to start it: this creates a "coordinator" space and starts the coordinator agent.'; click='up'; }
 else if(state==='degraded') { const reasons=status.degraded ?? []; text=`Degraded${reasons.length?` · ${reasonText(reasons[0].code)}${reasons.length>1?` +${reasons.length-1}`:''}`:''}${c.needs_you?` · ${c.needs_you} needs you`:''}`; compactText='Degraded'; tooltip=`Coordinator degraded: ${degradedSentence(status)}. Click to repair (coord up).`; click='up'; }
 else if(state==='starting') { text='Starting…'; compactText='…'; tooltip='Starting the coordinator…'; click='none'; }
 else if(state==='loading') { text='Coordinator …'; compactText='…'; tooltip='Reading coordinator status…'; }
 else { state='unavailable'; text='Coordinator unavailable'; compactText='n/a'; tooltip=`Coordinator status unavailable: ${clip(oneLine((ctx.error ?? '').split('\n')[0]),120)}. Click to retry.`; }
 if(c.needs_you>0 && state!=='running') tooltip+=` ${c.needs_you} needs you.`;
 if(stale) { text+=' ~'; compactText+=' ~'; tooltip+=` Last updated ${formatClock(status.generated_at_ms).slice(0,5)}.`; }
 // Room for text beside the icon: item padding, border, icon and gap take 41px.
 const inner=(ctx.width ?? 280)-41;
 let tier=estimateWidth(text)<=inner?'full':estimateWidth(compactText)<=inner?'dots':'icon', shown=segments;
 // When every count does not fit, keep the one that needs the person.
 if(tier==='icon' && state==='running' && c.needs_you>0 && estimateWidth(`● ${c.needs_you}`)<=inner) { tier='dots'; shown=segments.filter(s=>s.key==='needs_you'); compactText=`● ${c.needs_you}`; }
 return {state,tier,icon:({running:'coord-running.svg',not_running:'coord-off.svg',degraded:'coord-degraded.svg',starting:'coord-starting.svg'})[state] ?? 'coord-off.svg',iconRole:c.needs_you>0?'blocked':({running:'accent',degraded:'working'})[state] ?? 'dim',segments:shown,text,compactText,tooltip,label:tooltip,click,stale};
}
