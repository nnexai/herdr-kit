import {test} from 'node:test';
import assert from 'node:assert/strict';
import {railRows,railModel,quickReply,sayArgs,focusAction} from '../mods/kit-coord-ui/model.js';

const status = {enabled:true,state:'running',coordinator:{terminal:'c1'},counts:{open_workstreams:2},generated_at_ms:1000};
const entry = (seq,kind,data,more = {}) => ({seq,ts_ms:seq*1000,kind,data,...more});
const question = entry(3,'question.asked',{qid:'q1',text:'Queue the install?',options:['Queue','Hold']});
const conversation = {coordinator:{terminal:'c1',status:'idle'},source:{state:'ready'},messages:[
  {id:'u1',ts_ms:1000,role:'user',text:'Keep the raw terminal.'},
  {id:'a2',ts_ms:2000,role:'assistant',text:'**Yes.** It stays available.'},
]};
test('conversation text and key journal events interleave chronologically without transcript internals',() => {
  const log = {entries:[entry(5,'task.reported',{outcome:'done',summary:'Delivered'}),question,
    entry(4,'watcher.heartbeat',{summary:'not a rail event'})]};
  const rows = railRows(log,conversation,{workstreams:[]});
  assert.deepEqual(rows.map(row => row.kind),['person','reply','question','report']);
  assert.equal(rows[1].text,'**Yes.** It stays available.');
  assert.equal(rows[3].text,'Delivered');
});
test('question chips fill a draft; only a confirmed journal answer collapses the question',() => {
  const before = railRows({entries:[question]},conversation,null).find(row => row.kind === 'question');
  const selection = quickReply(before,'Queue');
  assert.deepEqual(sayArgs(selection.text,selection.question),['say','--text','Queue','--question','q1']);
  assert.equal(before.answer,undefined);
  assert.equal(quickReply(before,'Not an option'),null);
  const after = railRows({entries:[question,entry(6,'question.answered',{qid:'q1',text:'Queue'})]},conversation,null).find(row => row.kind === 'question');
  assert.equal(after.answer,'Queue'); assert.equal(quickReply(after,'Queue'),null);
});
test('pending questions outside the journal page remain answerable from Board',() => {
  const board = {workstreams:[],attention:[{kind:'question',qid:'old-q',seq:2,since_ms:100,text:'Still pending?',options:['Yes']}]};
  const vm = railModel({status,conversation,log:{entries:[]},board});
  assert.equal(vm.questions[0].questionId,'old-q');
  assert.deepEqual(quickReply(vm.questions[0],'Yes').text,'Yes');
  assert.equal(vm.rows[0].kind,'question');
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
