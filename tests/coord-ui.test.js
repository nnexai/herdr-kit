import {test} from 'node:test';
import assert from 'node:assert/strict';
import {railRows,railModel,quickReply,sayArgs,focusAction,groupDays,dayLabel,journalRows,journalFields,
  decodeRoles,parseCliError,filterRoles,roleSummaryLine,roleCreateArgs,roleEditArgs,roleDeleteArgs,roleResetArgs} from '../mods/kit-coord-ui/model.js';

const status = {enabled:true,state:'running',coordinator:{terminal:'c1'},counts:{open_workstreams:2},generated_at_ms:1000};
const entry = (seq,kind,data,more = {}) => ({seq,ts_ms:seq*1000,kind,data,...more});
const question = entry(3,'question.asked',{qid:'q1',text:'Queue the install?',options:['Queue','Hold']});
const conversation = {coordinator:{terminal:'c1',status:'idle'},source:{state:'ready'},messages:[
  {id:'u1',ts_ms:1000,role:'user',text:'Keep the raw terminal.'},
  {id:'a2',ts_ms:2000,role:'assistant',text:'**Yes.** It stays available.'},
]};
test('receipts anchor to the latest preceding ask and reports echo after later stream items',() => {
  const board = {open_questions:[],attention:[],workstreams:[
    {id:'w1',title:'Calendar',state:'open',opened_ms:1500,space:{id:'s1',name:'Plans'},tasks:[{id:'t1',state:'reported',report:'done',summary:'Three mocks',last_event_ms:5000,terminal:'term',tab_id:'tab'}]},
    {id:'w2',title:'Earlier',state:'open',opened_ms:500,tasks:[]},
  ]};
  const log={entries:[entry(1,'task.placed',{reason:'same space'},{ws:'w1',task:'t1'}),
    entry(3,'task.progress',{summary:'Sketching'},{ws:'w1',task:'t1'}),
    entry(5,'task.reported',{outcome:'done',summary:'Three mocks'},{ws:'w1',task:'t1'}),
    entry(6,'decision',{what:'Pick A',why:'clearer'},{ws:'w1'}),
    entry(7,'task.progress',{summary:'Other work'},{ws:'w2'})]};
  const rows=railRows(log,conversation,board,9000);
  assert.deepEqual(rows.map(r=>r.kind),['receipt','person','receipt','reply','echo']);
  const receipt=rows[2];
  assert.equal(receipt.anchor,'message:u1'); assert.equal(receipt.opened_ms,1500);
  assert.equal(receipt.state,'done'); assert.equal(receipt.text,'Three mocks');
  assert.equal(receipt.space,'Plans'); assert.equal(receipt.duration,'3s');
  assert.deepEqual(receipt.links.map(x=>x.kind),['terminal','space']);
  assert.deepEqual(receipt.trace.map(x=>x.text),['Placed — same space','Progress: Sketching','Reported done — Three mocks','Pick A — clearer']);
  assert.equal(rows[4].receiptKey,receipt.key); assert.equal(rows[4].text,'Three mocks');
  assert.equal(rows[3].text,'**Yes.** It stays available.');
});
test('reports do not echo until a later stream item exists',() => {
  const board={open_questions:[],workstreams:[{id:'w',title:'One',state:'open',opened_ms:1500,tasks:[]}]};
  const log={entries:[entry(2,'task.reported',{outcome:'done',summary:'Done'},{ws:'w'})]};
  assert.equal(railRows(log,{messages:[conversation.messages[0]]},board).some(r=>r.kind==='echo'),false);
});
test('needs-you uses complete authoritative questions, deduplicates linked attention and preserves independent blocks',() => {
  const board={workstreams:[{id:'w',title:'Project',opened_ms:1000,state:'open',tasks:[{id:'t',state:'reported',report:'question'}]}],
    open_questions:[{qid:'q1',ws:'w',task:'t',since_ms:3000,text:'Queue the install?',options:['Queue','Hold']},
      {qid:'q2',ws:'w',task:'t',since_ms:4000,text:'Another?',options:['Yes']}],
    attention:[{id:'b',kind:'task_question',ws:'w',task:'t',qid:'q1',since_ms:2500,text:'Delegate blocked'},
      {id:'b2',kind:'task_question',ws:'w',task:'t',since_ms:2600,text:'Same question'},
      {id:'s',kind:'stalled',ws:'w',task:'other',since_ms:5000,text:'Stalled'}]};
  const vm=railModel({status,conversation,board,log:{entries:[]},now:6000});
  assert.deepEqual(vm.questions.map(q=>q.questionId),['q1','q2']);
  assert.deepEqual(vm.needsYou.map(q=>q.kind),['question','question','attention']);
  assert.equal(vm.needsYou[0].title,'Project');
  assert.equal(vm.rows.find(r=>r.kind==='receipt').text,'Waiting on you ↑');
  const selection=quickReply(vm.questions[0],'Queue');
  assert.deepEqual(sayArgs(selection.text,selection.question),['say','--text','Queue','--question','q1']);
  assert.equal(quickReply(vm.questions[0],'Not an option'),null);
  const after=railModel({status,conversation,board:{...board,open_questions:[],attention:[]},log:{entries:[question]},now:6000});
  assert.deepEqual(after.questions,[]); assert.equal(after.step,'Ready when you are');
});
test('a closed question cannot leave a receipt waiting on the person',() => {
  const ws={id:'w',title:'Work',opened_ms:1000,state:'open',tasks:[{id:'t',state:'reported',report:'question',summary:'Needs a decision'}]};
  const stale={id:'a',kind:'task_question',ws:'w',task:'t',since_ms:2000,text:'Delegate is blocked'};
  const model=attention=>railModel({status,conversation,board:{workstreams:[ws],open_questions:[],attention},log:{entries:[]},now:6000});
  const withAttention=model([stale]);
  assert.equal(withAttention.questions.length,0);
  assert.equal(withAttention.needsYou[0].kind,'attention');
  assert.equal(withAttention.rows.find(r=>r.kind==='receipt').state,'blocked');
  assert.notEqual(withAttention.rows.find(r=>r.kind==='receipt').text,'Waiting on you ↑');
  const withoutAttention=model([]);
  assert.notEqual(withoutAttention.rows.find(r=>r.kind==='receipt').state,'question');
  assert.notEqual(withoutAttention.rows.find(r=>r.kind==='receipt').text,'Waiting on you ↑');
});
test('authoritative question fields survive bounded journal pages and can fall back to the matching event',() => {
  const board={workstreams:[],open_questions:[{qid:'q1'},{qid:'old',since_ms:100,text:'Still pending?',options:['Yes']}]};
  const vm=railModel({status,conversation,log:{entries:[question]},board,now:6000});
  assert.equal(vm.questions.find(q=>q.questionId==='q1').text,'Queue the install?');
  assert.equal(quickReply(vm.questions.find(q=>q.questionId==='old'),'Yes').text,'Yes');
});
test('receipt uses current task state, progress, blocked reason and fallback placement',() => {
  const task={id:'t',state:'running',last_event_ms:4000,terminal:'term'};
  const ws={id:'w',title:'Work',opened_ms:1000,state:'open',space:{id:'s',name:'Space'},tasks:[task]};
  const log={entries:[entry(3,'task.progress',{summary:'Making progress'},{ws:'w',task:'t'})]};
  const receipt=(board,events=log)=>railRows(events,{messages:[]},board,5000)[0];
  assert.equal(receipt({workstreams:[ws]}).text,'Making progress');
  assert.equal(receipt({workstreams:[ws]}, {entries:[]}).text,'Agent started in Space');
  const blocked=receipt({workstreams:[ws],attention:[{id:'x',kind:'stalled',ws:'w',task:'t',text:'No response',since_ms:4000}]});
  assert.equal(blocked.state,'blocked'); assert.equal(blocked.text,'No response');
  assert.equal(receipt({workstreams:[{...ws,tasks:[{...task,state:'placed',placement_reason:'Continue here'}]}]}).state,'placed');
  const finished=receipt({workstreams:[{...ws,tasks:[{...task,state:'closed',summary:'Delivered'}]}]},
    {entries:[entry(4,'task.reported',{outcome:'done',summary:'Delivered'},{ws:'w',task:'t'})]});
  assert.equal(finished.state,'done'); assert.equal(finished.text,'Delivered');
});
test('local day groups collapse old days and expand by stable day key',() => {
  const yesterday=new Date(2026,10,19,12).getTime(), today=new Date(2026,10,20,12).getTime();
  const rows=[{key:'a',kind:'person',ts_ms:yesterday},{key:'r',kind:'receipt',state:'done',ts_ms:yesterday},
    {key:'b',kind:'reply',ts_ms:today}];
  const collapsed=groupDays(rows,today);
  assert.deepEqual(collapsed.map(r=>r.kind),['day','day','reply']);
  assert.equal(collapsed[0].label,'Yesterday'); assert.equal(collapsed[0].count,1); assert.equal(collapsed[0].summary,'all done');
  const expanded=groupDays(rows,today,new Set([collapsed[0].day]));
  assert.deepEqual(expanded.map(r=>r.kind),['day','person','receipt','day','reply']);
  assert.equal(expanded[3].label,'Today');
});
test('journal groups newest-first events by day and workstream run, hiding system events by default',() => {
  const at = (seq,h,kind,data,more={}) => ({...entry(seq,kind,data,more),ts_ms:new Date(2026,10,20,h).getTime(),actor:'coord',v:1});
  const log={entries:[
    at(1,9,'ws.opened',{title:'Old title'},{ws:'w'}),
    at(2,10,'task.reported',{outcome:'done',summary:'Delivered',result:'/tmp/result'},{ws:'w',task:'t'}),
    at(3,11,'coord.session_started',{role:'coord-main'}),
    at(4,12,'watcher.started',{release:'abcdef0123'}),
    at(5,13,'ws.opened',{title:'Closed work'},{ws:'gone'}),
  ]};
  const now=new Date(2026,10,20,14).getTime();
  const rows=journalRows(log,{workstreams:[{id:'w',title:'Calendar'}]},{now});
  assert.deepEqual(rows.map(r=>r.kind==='event'?r.key:`${r.kind}:${r.label ?? r.title}`),
    ['day:Today','group:Closed work','journal:5','group:Calendar','journal:2','journal:1']);
  const all=journalRows(log,{workstreams:[]},{now,showSystem:true});
  assert.deepEqual(all.filter(r=>r.kind==='event').map(r=>r.key),['journal:5','journal:4','journal:3','journal:2','journal:1']);
  assert.deepEqual(all.filter(r=>r.kind==='group').map(r=>r.title),['Closed work','Coordinator','Old title']);
  const found=journalRows(log,{},{now,filter:'/tmp/result'}).filter(r=>r.kind==='event');
  assert.equal(found.length,1); assert.equal(found[0].source.seq,2); assert.equal(found[0].tone,'done');
  const partial=journalRows({entries:[
    at(6,8,'task.placed',{title:'Recovered name'},{ws:'early'}),at(7,9,'task.reported',{outcome:'done'},{ws:'early'}),
    at(8,10,'ws.closed',{summary:'Gone'},{ws:'unknown'})]},{},{now});
  assert.deepEqual(partial.filter(r=>r.kind==='group').map(r=>r.title),['Earlier workstream','Recovered name']);
});
test('working state and elapsed derive only from the real coordinator turn',() => {
  const working = {...conversation,coordinator:{status:'working',working_since_ms:10000,step:{tool:'bash',label:'coord task start'}}};
  const vm = railModel({status,conversation:working,now:75000});
  assert.equal(vm.working,true); assert.equal(vm.elapsed,'1:05'); assert.equal(vm.step,'coord task start');
  const idle = railModel({status,conversation,now:750000});
  assert.equal(idle.working,false); assert.equal(idle.elapsed,'now');
  const unknownTime = railModel({status,conversation:{...working,coordinator:{status:'working'}},now:75000});
  assert.equal(unknownTime.elapsed,'…');
});
test('off, missing and unavailable states never enable a stale send',() => {
  assert.equal(railModel({status:{...status,enabled:false},conversation}).available,false);
  const failed = railModel({status,conversation,error:'kit-coord is missing'});
  assert.equal(failed.available,false); assert.match(failed.step,/missing/);
  const missingSource = railModel({status,conversation:{...conversation,source:{state:'pending',detail:'Session is starting'}}});
  assert.equal(missingSource.step,'Session is starting');
});
test('messages are not trimmed, rewrapped or sent to a delegate target',() => {
  assert.deepEqual(sayArgs('  exact\n\ttext  '),['say','--text','  exact\n\ttext  ']);
  assert.equal(sayArgs(' \t\n'),null);
  assert.deepEqual(focusAction({kind:'space',target:'sp1',tab:'tab2'}),[
    {name:'space.select',args:{id:'sp1'}},{name:'tab.select',args:{id:'tab2'}}]);
  assert.deepEqual(focusAction({kind:'terminal',target:'t2v'}),[{name:'agent.focus',args:{terminal:'t2v'}}]);
});

test('closed task keeps its actual result even when its report field is cleared',() => {
  const ws={id:'w',title:'Calendar mocks',state:'open',opened_ms:1000,summary:null,
    tasks:[{id:'t',state:'closed',report:null,summary:'Three mocks and a spec',state_since_ms:5000}]};
  const events={entries:[entry(4,'task.reported',{outcome:'done',summary:'Three mocks and a spec'},{ws:'w',task:'t'}),
    entry(5,'task.closed',{outcome:'accepted',reason:'accepted'},{ws:'w',task:'t'})]};
  const receipt=railRows(events,{messages:[]},{workstreams:[ws]},6000)[0];
  assert.equal(receipt.state,'done'); assert.equal(receipt.text,'Three mocks and a spec');
  const unsuccessful=railRows({entries:[entry(5,'task.closed',{outcome:'superseded',reason:'Handed to escalation'},{ws:'w',task:'t'})]},
    {messages:[]},{workstreams:[ws]},6000)[0];
  assert.equal(unsuccessful.state,'blocked'); assert.equal(unsuccessful.text,'Handed to escalation');
});

test('duplicate workstreams and pre-opening messages do not create phantom receipts or echoes',() => {
  const ws={id:'w',title:'Work',state:'open',opened_ms:3000,tasks:[{id:'t',state:'reported',report:'done',summary:'Delivered'}]};
  const messages=[{id:'a',role:'assistant',ts_ms:500,text:'Okay'}, {id:'u',role:'user',ts_ms:1000,text:'Start'}];
  const reports=[entry(4,'task.reported',{outcome:'done',summary:'Delivered'},{ws:'w',task:'t'}),
    entry(5,'task.reported',{outcome:'done',summary:'Delivered'},{ws:'w',task:'t'})];
  const board={workstreams:[ws,ws]};
  const before=railRows({entries:reports},{messages},board,6000);
  assert.equal(before.filter(r=>r.kind==='receipt').length,1);
  assert.equal(before.filter(r=>r.kind==='echo').length,0);
  const after=railRows({entries:reports},{messages:[...messages,{id:'b',role:'assistant',ts_ms:3500,text:'Update'}]},board,6000);
  assert.equal(after.filter(r=>r.kind==='echo').length,1);
});

test('collapsed historical group identifies retained work when its ask is outside the conversation',() => {
  const old=new Date(2026,10,14,12).getTime(), now=new Date(2026,10,20,12).getTime();
  const rows=[{key:'r',kind:'receipt',state:'done',ts_ms:old},{key:'a',kind:'reply',ts_ms:now}];
  const groups=groupDays(rows,now);
  assert.equal(groups[0].collapsed,true); assert.equal(groups[0].countLabel,'1 workstream');
  assert.equal(groups[0].summary,'all done');
  assert.equal(groupDays(rows,now,new Set([groups[0].day]))[1].key,'r');
  assert.equal(dayLabel(now,now),'Today'); assert.equal(dayLabel(old,now),'Sat Nov 14');
  assert.equal(dayLabel(new Date(2025,10,14,12).getTime(),now),'Fri Nov 14, 2025');
});

test('journal sentences are plain language; reasons and internals move to the expanded fields',() => {
  const decision=entry(1,'decision',{tier:'silent',what:'Deploy on main',why:'Tree is clean'},{ws:'w',actor:'coord',v:1});
  const placed=entry(2,'task.placed',{title:'Deploy',rung:3,role:'coord-deep',reason:'Deploys run on the main checkout'},{ws:'w',task:'w-t1',actor:'coord',v:1});
  const blocked=entry(3,'task.reported',{outcome:'blocked',summary:'Needs approval'},{ws:'w',task:'w-t1',actor:'delegate',v:1});
  const rows=journalRows({entries:[decision,placed,blocked]},{}).filter(r=>r.kind==='event');
  assert.deepEqual(rows.map(r=>r.text),['Blocked: Needs approval','Started task “Deploy”: Deploys run on the main checkout','Decided: Deploy on main']);
  assert.deepEqual(rows.map(r=>r.tone),['blocked','quiet','quiet']);
  assert.ok(rows.every(r=>!/silent|rung|coord-deep|w-t1/.test(r.text)));
  const fields=Object.fromEntries(journalFields(rows[2]));
  assert.equal(fields.Why,'Tree is clean'); assert.match(fields.Event,/^decision · coord · #1$/);
  assert.equal(Object.fromEntries(journalFields(rows[1])).Task,'w-t1');
});

test('historical journal workstreams produce receipts with closed outcomes and board precedence',() => {
  const log={entries:[
    entry(1,'ws.opened',{title:'Historical'},{ws:'old'}),
    entry(2,'task.reported',{outcome:'done',summary:'Report outcome'},{ws:'old',task:'old-t1'}),
    entry(3,'ws.closed',{summary:'Closed outcome'},{ws:'old'}),
    entry(4,'ws.opened',{title:'Fallback'},{ws:'fallback'}),
    entry(5,'task.reported',{outcome:'done',summary:'Fallback outcome'},{ws:'fallback',task:'fallback-t1'}),
    entry(6,'ws.closed',{}, {ws:'fallback'}),
    entry(7,'ws.opened',{title:'Old title'},{ws:'live'}),
  ]};
  const board={open_questions:[],workstreams:[{id:'live',title:'Board title',opened_ms:7000,state:'open',tasks:[{id:'t',state:'running',summary:'Live progress'}]}]};
  const receipts=railRows(log,{messages:[]},board,10000).filter(row=>row.kind==='receipt');
  assert.equal(receipts.length,3);
  const old=receipts.find(row=>row.ws==='old');
  assert.equal(old.title,'Historical'); assert.equal(old.state,'done'); assert.equal(old.text,'Closed outcome');
  assert.equal(old.duration,'2s'); assert.deepEqual(old.trace.map(row=>row.text),['Reported done — Report outcome','Closed — Closed outcome']);
  assert.equal(receipts.find(row=>row.ws==='fallback').text,'Fallback outcome');
  assert.equal(receipts.find(row=>row.ws==='live').title,'Board title');
  assert.equal(receipts.find(row=>row.ws==='live').state,'running');
});

test('journal person messages survive session changes and merge with conversation copies',() => {
  const log={entries:[
    entry(1,'ws.opened',{title:'Unanchored'},{ws:'first'}),
    entry(2,'coord.sent',{text:'  Build   a view  '}),
    entry(3,'ws.opened',{title:'Anchored'},{ws:'second'}),
    entry(4,'coord.sent',{text:'Another ask'}),
    entry(5,'ws.opened',{title:'Another'},{ws:'third'}),
  ]};
  const conversation={messages:[{id:'current',role:'user',ts_ms:2100,text:'build a view'}]};
  const rows=railRows(log,conversation,{workstreams:[],open_questions:[]},10000);
  assert.equal(rows.filter(row=>row.kind==='person').length,2);
  assert.equal(rows.find(row=>row.ws==='first').anchor,undefined);
  assert.equal(rows.find(row=>row.ws==='second').anchor,'message:current');
  assert.equal(rows.find(row=>row.ws==='third').anchor,'sent:4');
  assert.equal(rows.find(row=>row.kind==='person').text,'build a view');
});

test('old asks do not pull later workstreams across days; recent asks still anchor',() => {
  const date=(day,hour,minute) => new Date(2026,10,day,hour,minute).getTime();
  const messages=[{id:'nov14',role:'user',text:'First ask',ts_ms:date(14,15,0)},
    {id:'nov16',role:'user',text:'Second ask',ts_ms:date(16,15,49)},
    {id:'recent',role:'user',text:'Recent ask',ts_ms:date(18,0,8)}];
  const workstreams=[
    {id:'guide',title:'kit-coord integration guide',opened_ms:date(16,9,11),state:'open',tasks:[]},
    {id:'resume',title:'Resume interrupted OMP sessions',opened_ms:date(18,0,7),state:'open',tasks:[]},
    {id:'recent',title:'Recent work',opened_ms:date(18,0,18),state:'open',tasks:[]},
  ];
  const rows=railRows({entries:[]},{messages},{workstreams},date(20,12,0));
  assert.equal(rows.find(r=>r.ws==='guide').ts_ms,date(16,9,11));
  assert.equal(rows.find(r=>r.ws==='guide').anchor,undefined);
  assert.equal(rows.find(r=>r.ws==='resume').ts_ms,date(18,0,7));
  assert.equal(rows.find(r=>r.ws==='resume').anchor,undefined);
  assert.equal(rows.find(r=>r.ws==='recent').anchor,'message:recent');
  const days=groupDays(rows,date(20,12,0));
  assert.ok(days.some(d=>d.label==='Wed Nov 18'));
});

test('collapsed days count workstreams and summarize blocked states',() => {
  const day=new Date(2026,10,16,12).getTime(), now=new Date(2026,10,20,12).getTime();
  const rows=[{key:'ask',kind:'person',ts_ms:day},
    ...['done','done','blocked'].map((state,i)=>({key:`r${i}`,kind:'receipt',state,ts_ms:day}))];
  const group=groupDays(rows,now)[0];
  assert.equal(group.label,'Mon Nov 16'); assert.equal(group.countLabel,'3 workstreams');
  assert.equal(group.summary,'1 blocked');
});

test('echo needs a later message and uses only the latest report for the workstream',() => {
  const board={workstreams:[
    {id:'guide',title:'Integration guide',opened_ms:1000,state:'open',tasks:[]},
    {id:'other',title:'Other receipt',opened_ms:2000,state:'open',tasks:[]},
    {id:'mission',title:'Mission Log fixes',opened_ms:3000,state:'open',tasks:[]},
  ]};
  const entries=[
    entry(4,'task.reported',{outcome:'done',summary:'Guide delivered'},{ws:'guide',task:'guide-t1'}),
    entry(6,'task.reported',{outcome:'done',summary:'First result'},{ws:'mission',task:'mission-t1'}),
    entry(8,'task.reported',{outcome:'done',summary:'Final result'},{ws:'mission',task:'mission-t2'}),
  ];
  const messages=[{id:'update',role:'assistant',text:'Update',ts_ms:5000}];
  const echoes=railRows({entries},{messages},board,9000).filter(row=>row.kind==='echo');
  assert.equal(echoes.length,1);
  assert.equal(echoes[0].ws,'mission'); assert.equal(echoes[0].text,'Final result'); assert.equal(echoes[0].seq,undefined);
  assert.equal(echoes[0].source.seq,8);
});

test('roles model builds CLI args for create/from/edit/delete/reset, filters roles, and extracts CLI errors',() => {
  const roles = decodeRoles(JSON.stringify({roles:[
    {name:'agy-default',harness:'agy',model:'gemini-4-neon-high',effort:null,source:'builtin',session:'agy-conversation'},
    {name:'claude-opus',harness:'claude',model:'opus',effort:'high',source:'override',session:'preset'},
    {name:'my-review',harness:'omp',model:'@advisor',effort:null,source:'file',session:'dir'},
  ]}));
  assert.equal(roles.length,3);
  assert.deepEqual(filterRoles(roles,'override').map(r => r.name),['claude-opus']);
  assert.deepEqual(filterRoles(roles,'agy').map(r => r.name),['agy-default']);
  assert.equal(roleSummaryLine(roles[1]),'claude · opus · effort: high');
  assert.equal(roleSummaryLine(roles[0]),'agy · gemini-4-neon-high');
  assert.equal(parseCliError(JSON.stringify({error:{code:'role_invalid',message:'missing {brief}'}})),'missing {brief}');
  assert.equal(parseCliError('plain error\nsecond line'),'plain error');

  assert.equal(roleCreateArgs({name:'  '}),null);
  assert.deepEqual(roleCreateArgs({
    name:'agy-fast',from:'agy-default',harness:'agy',model:'gemini-4-neon-medium',effort:'',
    session:'agy-conversation',start:'agy -i {brief}',resume:'',trust:'',body:'Fast delegate.',
  }),['roles','create','agy-fast','--from','agy-default','--harness','agy','--model','gemini-4-neon-medium',
    '--clear-effort','--session','agy-conversation','--start','agy -i {brief}','--clear-resume','--clear-trust',
    '--prompt','Fast delegate.']);
  assert.deepEqual(roleCreateArgs({
    name:'blank-omp',from:'',harness:'omp',model:'@task',effort:'',session:'dir',
    start:'omp --session-dir {session_dir} {brief}',resume:'',trust:'',body:'Prompt',
  }),['roles','create','blank-omp','--harness','omp','--model','@task','--session','dir',
    '--start','omp --session-dir {session_dir} {brief}','--prompt','Prompt']);
  assert.deepEqual(roleEditArgs('agy-default',{
    harness:'agy',model:'gemini-4-neon-high',effort:'high',session:'agy-conversation',
    start:'agy -i {brief}',resume:'agy --conversation {session_id} -i {brief}',trust:'',body:'Updated',
  }),['roles','edit','agy-default','--harness','agy','--model','gemini-4-neon-high','--effort','high',
    '--session','agy-conversation','--start','agy -i {brief}',
    '--resume','agy --conversation {session_id} -i {brief}','--clear-trust','--prompt','Updated']);
  assert.deepEqual(roleDeleteArgs('my-review'),['roles','delete','my-review']);
  assert.deepEqual(roleResetArgs('claude-opus'),['roles','reset','claude-opus']);
});
