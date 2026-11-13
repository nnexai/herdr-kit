import {test} from 'node:test';
import assert from 'node:assert/strict';
import * as m from '../mods/herdr-kit-coord/model.js';
const now=1763042861000;
const status=(x={})=>({schema_version:1,generated_at_ms:now,release:'a1b2c3d4',state:'running',enabled:true,autostart:true,degraded:[],watcher:{running:true,terminal:'t5',heartbeat_ms:now-4000,release:'a1b2c3d4'},coordinator:{terminal:'t4',status:'working',space_id:'sp',tab_id:'tb',session_started_ms:now-1000},counts:{running:3,needs_you:1,retained:2,open_workstreams:3},journal_dir:'/journal',journal_seq:42,...x});
const task=(x={})=>({id:'ws-a-t1',number:1,title:'Task',state:'running',report:null,start_blocked:false,stalled:false,state_since_ms:now-60000,placed_ms:now-60000,started_ms:now-50000,last_event_ms:now-1000,rung:2,placement_reason:'same files',rejected_rungs:[{rung:1,reason:'exhausted'}],role:'coord-default',harness:'omp',model:'luna',summary:null,terminal:'t7',space_id:'sp',tab_id:'tb',brief_path:'/brief',result_path:null,nudges:0,...x});
const ws=(x={})=>({id:'ws-a',title:'Fix login',state:'open',repo:{path:'/repo',name:'repo'},delivery_target:'main',summary:null,opened_ms:now-60000,closed_ms:null,last_event_ms:now-1000,space:{id:'sp',name:'repo',owned:false},retained:null,tasks:[task()],...x});
const att=(x={})=>({id:'q1',kind:'question',ws:'ws-a',task:'ws-a-t1',since_ms:now-1000,text:'Fix?',options:['Yes'],seq:1,...x});
const board=(x={})=>({schema_version:1,generated_at_ms:now,journal_seq:42,workstreams:[ws()],attention:[att()],...x});
const entry=(kind='decision',data={},x={})=>({v:1,seq:1,ts_ms:now,actor:'coordinator',kind,ws:'ws-a',task:'ws-a-t1',data,...x});
const log=(x={})=>({schema_version:1,generated_at_ms:now,entries:[entry()],has_more:false,newest_seq:1,oldest_seq:1,...x});
const json=JSON.stringify;
test('1 strict decoders preserve protocol and enforce caps',()=>{ for(const [decode,value] of [[m.decodeStatus,status()],[m.decodeBoard,board()],[m.decodeLog,log()]]) {assert.deepEqual(decode(json(value)),value); assert.throws(()=>decode(json({...value,schema_version:2})),/malformed/); assert.throws(()=>decode(json({...value,generated_at_ms:-1})),/malformed/); assert.throws(()=>decode('x'),/malformed/); } const s=status({email:'private',state:'future'}); assert.equal(m.decodeStatus(json(s)).email,undefined); assert.equal(m.decodeStatus(json(s)).state,'future'); const b=board(); b.workstreams[0].tasks[0].email='private'; assert.equal(m.decodeBoard(json(b)).workstreams[0].tasks[0].email,undefined); assert.throws(()=>m.decodeBoard(json(board({workstreams:Array(201).fill(ws())})))); assert.throws(()=>m.decodeBoard(json(board({workstreams:[ws({tasks:{}})]})))); assert.throws(()=>m.decodeLog(json(log({entries:Array(501).fill(entry())})))); });
test('2 status bar states, widths, stale and actions',()=>{ const vm=(s,c={})=>m.statusbarModel(s,{now,width:280,...c}); assert.equal(vm(status()).text,'3 running · 1 needs you · 2 retained'); assert.equal(vm(status()).tier,'full'); assert.equal(vm(status(),{width:150}).tier,'dots'); assert.equal(vm(status(),{width:60}).tier,'icon'); assert.equal(vm(status({counts:{running:999,needs_you:999,retained:999,open_workstreams:1}})).tier,'dots'); assert.equal(vm(status({counts:{running:0,needs_you:0,retained:0,open_workstreams:0}})).text,'No tasks'); assert.equal(vm(status({state:'not_running'})).click,'up'); assert.match(vm(status({state:'degraded',degraded:[{code:'watcher_missing',since_ms:null,detail:null}]})).text,/1 needs you/); assert.equal(vm(status(),{starting:true}).click,'none'); assert.equal(vm(null).click,'reread'); assert.equal(vm(status()).click,'board'); assert.match(vm(status(),{failed:true,lastOkMs:now-100000}).text,/ ~$/); assert.match(vm(status()).tooltip,/Click to open the board/); });
test('3 autostart predicate',()=>{ assert.equal(m.shouldAutostart(status({state:'degraded'}),false),true); for(const s of [status({enabled:false,state:'degraded'}),status({autostart:false,state:'degraded'}),status()]) assert.equal(m.shouldAutostart(s,false),false); assert.equal(m.shouldAutostart(status({state:'degraded'}),true),false); });
test('4 board grouping, ordering and disclosures',()=>{ const a=ws({id:'ws-a',tasks:[task({id:'a2',number:2,state:'placed'}),task({id:'a1',number:1,state:'reported',report:'blocked'}),task({id:'a3',number:3,state:'closed'})]}); const b=ws({id:'ws-b',last_event_ms:now,tasks:[]}); const r=ws({id:'ws-r',retained:{reason_code:'dirty',reason:'Dirty',since_ms:now,checked_ms:null,auto_clears:true,checks:[]}}); const vm=m.boardModel(board({workstreams:[b,r,a],attention:[att({id:'late'}),att({id:'early',since_ms:now-5000})]}),status(),{now,showClosed:new Set(['ws-a'])}); assert.deepEqual(vm.sections.map(s=>s.id),['attention','workstreams','retained']); assert.equal(vm.sections[0].rows[0].key,'early'); assert.deepEqual(vm.sections[1].rows.map(r=>r.key),['ws-a','a1','a2','ws-a:closed','a3','ws-b']); assert.equal(vm.sections[2].rows[0].key,'ws-r'); });
test('5 task states and placement',()=>{ for(const [x,word,role] of [[{state:'placed'},'starting','idle'],[{state:'placed',start_blocked:true},'stuck','blocked'],[{state:'running',stalled:true},'stalled','blocked'],[{},'running','working'],[{state:'reported',report:'done'},'done','done'],[{state:'reported',report:'blocked'},'blocked','blocked'],[{state:'reported',report:'question'},'asking','blocked'],[{state:'dormant'},'dormant','dim'],[{state:'closed'},'closed','dim'],[{state:'new'},'new','unknown']]) assert.deepEqual(m.taskRow(task(x),{now}).state,{word,role}); assert.match(m.taskRow(task(),{now}).meta,/rung 2 same space — same files/); assert.ok(m.taskRow(task({placement_reason:'x'.repeat(300)}),{now}).meta.length<=240); assert.equal(m.oneLine('a\n b'),'a b'); });
test('6 actions and effect descriptions',()=>{ const t=m.taskRow(task(),{now,status:status()}); assert.deepEqual(t.actions.map(a=>a.id),['show','peek','log']); assert.deepEqual(m.actionCommand('show',t,status()),{kind:'run',name:'agent.focus',args:{terminal:'t7'}}); assert.equal(m.actionCommand('peek',t,status()).name,'terminal.peek'); const dormant=m.taskRow(task({state:'dormant',terminal:null}),{now}); assert.deepEqual(dormant.actions.map(a=>a.id),['show-space','log']); assert.equal(m.actionCommand('show-space',dormant,status()).then.name,'tab.select'); const q=m.attentionRow(att(),{now,status:status()}); assert.equal(q.actions[0].id,'reply'); assert.deepEqual(m.attentionRow(att(),{now,status:status({coordinator:{terminal:null}})}).actions.map(a=>a.id),['log']); assert.equal(m.actionCommand('log',t,status()).kind,'log'); assert.equal(m.actionCommand('up',t,status()).kind,'up'); assert.equal(m.actionCommand('unknown',t,status()),null); });
test('attention kinds use human glyphs and origins, preserving unknown kinds',()=>{ const expected={question:['?','asked by coordinator'],task_question:['?','asked by delegate'],task_blocked:['!','delegate blocked'],start_blocked:['!','did not start'],stalled:['!','no report'],coordinator_blocked:['!','coordinator is blocked']}; for(const [kind,[glyph,origin]] of Object.entries(expected)){const row=m.attentionRow(att({kind,text:'Please review'}),{now});assert.equal(row.title,'Please review');assert.deepEqual(row.state,{word:glyph,role:'blocked'});assert.ok(row.meta.includes(origin));}const row=m.attentionRow(att({kind:'future_kind',text:'Please review'}),{now});assert.deepEqual(row.state,{word:'future_kind',role:'dim'});assert.match(row.meta,/future_kind/);});
test('timeline rendering window follows selection without discarding loaded rows',()=>{const rows=Array.from({length:450},(_,i)=>({key:String(i),selectable:true}));assert.deepEqual(m.timelineWindow(rows,'0'),{start:0,end:120,newer:false,older:true});assert.deepEqual(m.timelineWindow(rows,'200'),{start:81,end:201,newer:true,older:true});assert.deepEqual(m.timelineWindow(rows,'449'),{start:330,end:450,newer:true,older:false});assert.deepEqual(m.timelineWindow(rows,'20',330),{start:20,end:140,newer:true,older:true});assert.deepEqual(m.timelineWindow([],null),{start:0,end:0,newer:false,older:false});});
test('7 navigation clamps and reconciles',()=>{ const rows=[{key:'h',selectable:false},{key:'a',selectable:true},{key:'b',selectable:true},{key:'c',selectable:true}]; assert.equal(m.moveSelection(rows,'a','up',2),'a'); assert.equal(m.moveSelection(rows,'a','pagedown',2),'c'); assert.equal(m.moveSelection(rows,'c','down',2),'c'); assert.equal(m.moveSelection(rows,'c','home',2),'a'); assert.equal(m.reconcileSelection(rows,'b',1),'b'); assert.equal(m.reconcileSelection(rows,'missing',2),'b'); });
test('8 event summaries cover every known kind and unknown kinds',()=>{ const expected={'ws.opened':'Opened workstream','ws.closed':'Closed workstream','ws.cleaned':'Cleaned up','ws.retained':'Retained','task.placed':'Placed','task.started':'Started','task.reported':'Reported','task.progress':'Progress','task.closed':'Closed task','task.dormant':'Agent ended','task.start_blocked':'Did not start','task.stalled':'Stalled','res.intent':'Intent','res.created':'Created','res.adopted':'Adopted','res.release_checked':'Release check','res.released':'Released','nudge.sent':'Nudged','reconcile.finding':'Reconcile','watcher.started':'Watcher started','watcher.stopped':'Watcher stopped','coord.session_started':'Coordinator session started','coord.session_ended':'Coordinator session ended','inbox.drained':'Inbox drained','decision':'Decision','question.asked':'Asked','question.answered':'Answered','scout.result':'Scout','instruction.set':'Standing instruction','thread.opened':'Opened thread','thread.closed':'Closed thread','correction':'You corrected'}; for(const [kind,prefix] of Object.entries(expected)) assert.ok(m.summarize(entry(kind)).text.startsWith(prefix),kind); assert.equal(m.summarize(entry('future',{summary:'hello'})).text,'future — hello'); assert.equal(m.summarize(entry('ws.closed')).text,'Closed workstream'); });
test('9 timeline grouping, filtering, hidden count, scope and cap',()=>{ const entries=[entry('decision',{what:'Hello'},{seq:3}),entry('nudge.sent',{}, {seq:2,ts_ms:now-86400000}),entry('question.asked',{text:'Test'},{seq:1,ts_ms:now-3*86400000})]; const vm=m.timelineModel(log({entries,has_more:true}),{now,scope:{ws:'ws-a'}}); assert.equal(vm.hiddenCount,1); assert.equal(vm.scopeChip,'Scope: ws-a'); assert.equal(vm.rows[0].title,'TODAY'); assert.equal(vm.rows.at(-1).kind,'more'); assert.equal(m.timelineModel(log({entries}),{now,text:'hello'}).rows.filter(x=>x.kind==='entry').length,1); const all=m.timelineModel(log({entries}),{now,detail:'all'}); assert.ok(all.rows.some(r=>r.title==='YESTERDAY')); const many=Array.from({length:2001},(_,i)=>entry('decision',{}, {seq:i})); assert.match(m.timelineModel(log({entries:many,has_more:true}),{now}).rows.at(-1).title,/2000/); });
test('10 log merging deduplicates and sorts',()=>{ const e=n=>entry('decision',{}, {seq:n}); assert.deepEqual(m.mergeLog([e(3),e(2)],[e(4),e(3)],'prepend').map(x=>x.seq),[4,3,2]); assert.deepEqual(m.mergeLog([e(3)],[e(1),e(2)],'append').map(x=>x.seq),[3,2,1]); assert.deepEqual(m.mergeLog([e(3)],[e(1)],'replace').map(x=>x.seq),[1]); });
test('11 log command arguments',()=>{ assert.deepEqual(m.logArgs({},{}),['--json','--limit','200']); assert.deepEqual(m.logArgs({ws:'w'},{limit:5,beforeSeq:4}),['--json','--limit','5','--ws','w','--before-seq','4']); assert.deepEqual(m.logArgs({task:'t'},{afterSeq:0}),['--json','--limit','200','--task','t','--after-seq','0']); });
test('12 refresh coalescing',()=>{ assert.deepEqual(m.nextRefreshDelay({inFlight:true},1000),{waitMs:null}); assert.deepEqual(m.nextRefreshDelay({lastRunMs:500},1000),{waitMs:250}); assert.deepEqual(m.nextRefreshDelay({lastRunMs:250},1000),{runNow:true}); });
test('13 ages and local day labels',()=>{ assert.equal(m.formatAge(5000),'5s'); assert.equal(m.formatAge(720000),'12m'); assert.equal(m.formatAge(11100000),'3h 5m'); assert.equal(m.formatAge(100800000),'1d 4h'); assert.equal(m.formatDay(now,now),'TODAY'); const d=new Date(now); d.setDate(d.getDate()-1); assert.equal(m.formatDay(d.getTime(),now),'YESTERDAY'); });

test('status strip exposes every state and its accessible text',()=>{
 const vm=(s,c={})=>m.statusbarModel(s,{now,width:500,...c});
 const zero={running:0,needs_you:0,retained:0,open_workstreams:0};
 assert.equal(vm(status({counts:zero})).text,'No tasks');
 assert.equal(vm(status()).text,'3 running · 1 needs you · 2 retained');
 assert.equal(vm(status({state:'not_running'})).text,'Coordinator off');
 assert.match(vm(status({enabled:false})).tooltip,/creates a "coordinator" space/);
 const degraded=status({state:'degraded',degraded:[{code:'watcher_heartbeat_stale',detail:'watcher heartbeat stale for 3m 20s',since_ms:null},{code:'hc_unreachable',detail:null,since_ms:null}]});
 assert.equal(vm(degraded).text,'Degraded · watcher stalled +1 · 1 needs you');
 assert.match(vm(degraded).tooltip,/watcher heartbeat stale for 3m 20s/);
 assert.equal(vm(status(),{starting:true}).text,'Starting…');
 assert.equal(vm(null).text,'Coordinator …');
 assert.equal(vm(null,{failed:true,error:'read failed\nmore'}).text,'Coordinator unavailable');
 assert.equal(vm(null,{failed:true,error:'read failed\nmore'}).tooltip,'Coordinator status unavailable: read failed. Click to retry.');
 const stale=vm(status({generated_at_ms:now-100000}),{failed:true});
 assert.match(stale.text,/ ~$/);
 assert.match(stale.compactText,/ ~$/);
 assert.match(stale.tooltip,/ Last updated \d\d:\d\d\.$/);
 assert.equal(vm(status({generated_at_ms:now-121000}),{failed:true}).state,'unavailable');
});

test('board view exposes loading, empty, off, error and cached states',()=>{
 const empty=board({workstreams:[],attention:[]}), ui={now};
 assert.deepEqual(m.boardViewModel(null,status(),ui).empty,{kind:'loading',text:'Reading board…'});
 assert.equal(m.boardViewModel(empty,status(),ui).empty.kind,'empty');
 assert.equal(m.boardViewModel(empty,status(),ui).empty.action,undefined);
 assert.equal(m.boardViewModel(empty,status({enabled:false}),ui).empty.kind,'off');
 assert.equal(m.boardViewModel(empty,status({enabled:false}),ui).empty.action,undefined);
 assert.equal(m.boardViewModel(null,status(),{...ui,failed:true,error:'bad read\nextra'}).empty.action,'Retry');
 const cached=m.boardViewModel(board(),status(),{...ui,failed:true,error:'bad read\nextra'});
 assert.equal(cached.banner,`Showing data from ${m.formatClock(now).slice(0,5)} — refresh failed: bad read.`);
 assert.equal(cached.bannerAction,'Retry');
 assert.ok(cached.rows.length);
 const off=m.boardViewModel(board(),status({enabled:false}),ui);
 assert.equal(off.banner,'Coordinator is not running.');
 assert.equal(off.bannerAction,null);
 const degradedStatus=status({state:'degraded',degraded:[{code:'watcher_missing',detail:'watcher stopped',since_ms:null}]});
 const degraded=m.boardViewModel(board(),degradedStatus,ui);
 assert.equal(degraded.bannerAction,null);
 assert.equal(m.timelineViewModel(log(),{...ui,status:degradedStatus}).banner,degraded.banner);
 assert.equal(m.timelineViewModel(log(),{...ui,status:degradedStatus}).bannerAction,null);
 assert.equal(m.boardViewModel(board(),status(),{...ui,starting:true}).header.state,'starting');
});

test('timeline view exposes loading, empty, filtered, scoped and failure states',()=>{
 const ui={now};
 assert.deepEqual(m.timelineViewModel(null,ui).empty,{kind:'loading',text:'Reading journal…'});
 assert.deepEqual(m.timelineViewModel(log({entries:[]}),ui).empty,{kind:'empty',text:'The journal is empty.',detail:'Nothing has happened yet.'});
 assert.deepEqual(m.timelineViewModel(log({entries:[]}),{...ui,scope:{task:'ws-a-t1'}}).empty,{kind:'scope',text:'No entries for ws-a-t1.',action:'Clear scope'});
 assert.deepEqual(m.timelineViewModel(log(),{...ui,text:'missing'}).empty,{kind:'filter',text:'No entries match “missing”.',action:'Clear filter'});
 assert.deepEqual(m.timelineViewModel(null,{...ui,failed:true,error:'bad read\nextra'}).empty,{kind:'error',text:"Can't read the journal.",detail:'bad read',action:'Retry'});
 const cached=m.timelineViewModel(log(),{...ui,failed:true,error:'bad read'});
 assert.equal(cached.banner,`Showing data from ${m.formatClock(now).slice(0,5)} — refresh failed: bad read.`);
 assert.ok(cached.rows.some(row=>row.kind==='entry'));
 assert.equal(m.timelineViewModel(log(),{...ui,newerCount:2}).newerPill,'2 new entries ↑');
});

test('status failures remain visible independently of board and journal reads',()=>{
 const ui={now,statusFailed:true,statusError:'status failed\nignored',status:status()};
 const expected=`Showing data from ${m.formatClock(now).slice(0,5)} — refresh failed: status failed.`;
 assert.equal(m.boardViewModel(board(),status(),ui).banner,expected);
 assert.equal(m.timelineViewModel(log(),ui).banner,expected);
 assert.equal(m.timelineViewModel(log(),{...ui,status:null}).banner,'Coordinator status unavailable: status failed.');
 assert.match(m.boardViewModel(board(),status(),{...ui,failed:true,error:'board failed'}).banner,/board failed/);
 assert.match(m.timelineViewModel(log(),{...ui,failed:true,error:'journal failed'}).banner,/journal failed/);
});

test('timeline defers new entries away from top until explicitly accepted',()=>{
 const rows=[{key:'day',selectable:false},{key:'4',selectable:true},{key:'3',selectable:true}];
 assert.equal(m.shouldDeferTimeline(rows,'4'),false);
 assert.equal(m.shouldDeferTimeline(rows,null),false);
 assert.equal(m.shouldDeferTimeline(rows,'3'),true);
 assert.equal(m.shouldDeferTimeline(rows,'4',[entry()]),true);
 assert.equal(m.shouldDeferTimeline([],null),false);
 const e=n=>entry('decision',{}, {seq:n});
 const pending=m.mergePendingEntries([e(5),e(4)],[e(6),e(5),e(3)],[e(4),e(3)]);
 assert.deepEqual(pending.map(x=>x.seq),[6,5]);
 assert.deepEqual(m.mergeLog([e(4),e(3)],pending,'prepend').map(x=>x.seq),[6,5,4,3]);
 assert.equal(m.mergePendingEntries([],Array.from({length:2100},(_,i)=>e(i)),[]).length,2000);
});

test('running start-blocked and stalled tasks sort before running tasks',()=>{
 const tasks=[task({id:'running',number:1}),task({id:'stuck',number:3,start_blocked:true}),task({id:'stalled',number:2,stalled:true})];
 const rows=m.boardModel(board({workstreams:[ws({tasks})],attention:[]}),status(),{now}).sections[0].rows;
 assert.deepEqual(rows.slice(1).map(r=>[r.key,r.state.word,r.state.role]),[['stalled','stalled','blocked'],['stuck','stuck','blocked'],['running','running','working']]);
 assert.equal(m.taskRow(task({start_blocked:true,stalled:true}),{now}).state.word,'stuck');
});

test('refresh pages close gaps and expose a loadable gap at the cap',()=>{
 const e=n=>entry('decision',{}, {seq:n});
 const page=(hi,lo,more)=>log({entries:Array.from({length:hi-lo+1},(_,i)=>e(hi-i)),oldest_seq:lo,newest_seq:hi,has_more:more});
 let state=m.mergeRefreshPage({},page(550,351,true));
 assert.equal(state.beforeSeq,351);
 state=m.mergeRefreshPage(state,page(350,151,true));
 assert.equal(state.beforeSeq,151);
 state=m.mergeRefreshPage(state,page(150,101,false));
 assert.equal(state.beforeSeq,null);
 assert.equal(state.gap,false);
 assert.deepEqual(m.mergeLog([e(100)],state.entries,'prepend').map(x=>x.seq),Array.from({length:451},(_,i)=>550-i));
 state={};
 for(let hi=2500;hi>500;hi-=200) state=m.mergeRefreshPage(state,page(hi,hi-199,true));
 assert.equal(state.entries.length,2000);
 assert.equal(state.beforeSeq,null);
 assert.equal(state.gap,true);
 const vm=m.timelineModel(log({entries:state.entries,gap:true,has_more:true}),{now});
 assert.deepEqual([vm.rows.at(-1).kind,vm.rows.at(-1).selectable],['more',true]);
 assert.match(vm.rows.at(-1).title,/missing older/);
 const capped=m.mergeTimelinePage(log({entries:[e(100)],has_more:false}),{...state.latest,entries:state.entries},'prepend',state.gap);
 assert.equal(capped.gap,true);
 const older=m.mergeTimelinePage(capped,page(500,301,true),'append');
 assert.equal(older.gap,false);
 assert.equal(older.entries[0].seq,500);
 assert.equal(older.oldest_seq,301);
 assert.equal(older.has_more,true);
 assert.deepEqual(m.logArgs({ws:'w'},{beforeSeq:501,afterSeq:100}),['--json','--limit','200','--ws','w','--before-seq','501','--after-seq','100']);
});

test('filtered loaded pages keep older entries reachable',()=>{
 for(const ui of [{now,text:'absent'},{now,detail:'key'}]) {
  const value=log({entries:[entry('nudge.sent')],has_more:true});
  const vm=m.timelineViewModel(value,ui);
  assert.match(vm.empty.text,/loaded pages matches/);
  assert.equal(vm.rows.at(-1).kind,'more');
  assert.equal(vm.rows.at(-1).selectable,true);
 }
});

test('closed-task disclosure labels follow their count and retain navigation',()=>{
 for (const count of [1,2]) {
  const tasks=Array.from({length:count},(_,i)=>task({id:`ws-a-t${i+1}`,number:i+1,state:'closed'}));
  const ui={now,collapsed:new Set(['open:ws-a'])};
  const b=board({workstreams:[ws({tasks})],attention:[]});
  const collapsed=m.boardModel(b,status(),ui);
  const disclosure=collapsed.rows.find(row=>row.kind==='closed');
  assert.equal(disclosure.title,`${count} closed ${count===1?'task':'tasks'}`);
  assert.equal(m.moveSelection(collapsed.rows,'ws-a','down'),disclosure.key);
  const expanded=m.boardModel(b,status(),{...ui,showClosed:new Set(['ws-a'])});
  assert.deepEqual(expanded.rows.filter(row=>row.kind==='task').map(row=>row.key),tasks.map(t=>t.id));
 }
});
