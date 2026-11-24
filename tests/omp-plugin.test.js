import {test} from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import {replayRisk, settled} from '../omp-plugin/transcript.ts';

const user = text => ({type: 'message', message: {role: 'user', content: [{type: 'text', text}]}});
const answer = (stopReason = 'stop', content = [{type: 'text', text: 'done'}]) =>
  ({type: 'message', message: {role: 'assistant', stopReason, content}});
const toolCall = answer('toolUse', [{type: 'toolCall', id: 'c1', name: 'bash', arguments: {}}]);
const toolResult = {type: 'message', message: {role: 'toolResult', content: []}};
const exit = (kind, extra = {}) =>
  ({type: 'custom', customType: 'session_exit', data: {reason: 'dispose', kind, recordedAt: '2026-11-24T00:00:00.000Z', ...extra}});
const meta = {type: 'model_usage'};

test('only a finished answer or a recorded normal exit settles a transcript', () => {
  assert.equal(settled([user('hi'), answer(), meta]), true);
  assert.equal(settled([]), true);
  const unfinished = {
    prompt: [user('hi'), answer(), user('again')],
    toolCall: [user('hi'), toolCall],
    toolResult: [user('hi'), toolCall, toolResult],
    aborted: [user('hi'), answer('aborted')],
    error: [user('hi'), answer('error')],
    stopWithToolCall: [user('hi'), answer('stop', [{type: 'toolCall', id: 'c', name: 'x', arguments: {}}])],
    extensionPrompt: [user('hi'), answer(), {type: 'custom_message', customType: 'ext', content: 'go'}],
    crashExit: [user('hi'), toolCall, exit('process_exit')],
    signalExit: [user('hi'), toolCall, exit('signal')],
    exitWithoutTimestamp: [user('hi'), toolCall, {type: 'custom', customType: 'session_exit', data: {reason: 'dispose', kind: 'normal'}}],
    exitBeforeTail: [user('hi'), exit('normal'), toolCall],
  };
  for (const [name, entries] of Object.entries(unfinished)) assert.equal(settled(entries), false, name);
  assert.equal(settled([user('hi'), toolCall, exit('normal')]), true);
  assert.equal(settled([user('hi'), answer('aborted'), exit('normal'), meta]), true);
  // An idle eval snapshot after a finished answer does not start a turn.
  assert.equal(settled([user('hi'), answer(), {type: 'custom_message', customType: 'eval-state-context'}]), true);
});

test('a historical subagent anywhere below the session blocks a restart', t => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'hk-omp-guard-'));
  t.after(() => fs.rmSync(dir, {recursive: true, force: true}));
  const sessionFile = path.join(dir, 'main.jsonl');
  const lines = entries => [{type: 'session', id: 's'}, ...entries].map(e => JSON.stringify(e)).join('\n') + '\n';
  const done = [user('hi'), answer()];
  fs.writeFileSync(sessionFile, lines(done));
  assert.equal(replayRisk(done, sessionFile), undefined);
  assert.equal(replayRisk([...done, user('again')], sessionFile), 'restorable_tail');

  fs.mkdirSync(path.join(dir, 'main', 'Lead'), {recursive: true});
  fs.writeFileSync(path.join(dir, 'main', 'Lead.jsonl'), lines([user('task'), toolCall, toolResult, exit('normal')]));
  assert.equal(replayRisk(done, sessionFile), undefined);

  const nested = path.join(dir, 'main', 'Lead', 'Helper.jsonl');
  fs.writeFileSync(nested, lines([user('Complete assignment: work')]));
  assert.equal(replayRisk(done, sessionFile), 'restorable_subagent');

  fs.writeFileSync(nested, lines([user('task'), answer()]) + '{"type":"message",');
  assert.equal(replayRisk(done, sessionFile), 'restorable_subagent', 'a torn transcript counts as unfinished');
});
