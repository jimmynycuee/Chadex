import { describe, expect, it } from 'vitest';
import { canonical, FerretController, taskState, toolState } from '../src/ferret';
import { snapshot, task } from './fixtures';

describe('canonical ferret projection', () => {
  it('loads all canonical states, labels, poses and semantic mappings', () => {
    expect(canonical.states).toHaveLength(11);
    for (const state of canonical.states) { expect(canonical.labels[state]).toBeTruthy(); expect(canonical.poses[state].name).toBe(state); }
    expect(toolState('apply_project_edits')).toBe('coding'); expect(toolState('run_validation')).toBe('testing');
    expect(toolState('search_project_texts')).toBe('searching'); expect(toolState('run_process')).toBe('thinking');
    expect(canonical.ignoredTools).toContain('task_status'); expect(canonical.ignoredTools).toContain('observe_jobs');
    expect(taskState(task({ plan: ['run_process'] }))).toBe('thinking');
  });
  it('does not confuse waiting for connection verification with blocked work', () => {
    const ferret = new FerretController(); expect(ferret.update(snapshot(), [], 0).state).toBe('idle');
    expect(ferret.update(snapshot(), [], 180_000).state).toBe('sleep');
  });
  it('shows semantic live work and long tasks, without invented percentage', () => {
    const ferret = new FerretController(); const runtime = snapshot({ task_progress: task({ total_steps: 0 }) });
    expect(ferret.update(runtime, [], 0).state).toBe('listening');
    expect(ferret.update(runtime, [], 1_001).state).toBe('coding');
    expect(ferret.update(runtime, [], 60_000).state).toBe('longTask');
    expect(ferret.update(runtime, [], 61_000).progress).toBeNull();
    expect(taskState(task({ status: 'validating' }))).toBe('testing');
    expect(taskState(task({ cancel_requested: true }))).toBe('waiting');
  });
  it('does not replay historical completion or carry reactions across projects', () => {
    const ferret = new FerretController(); const complete = snapshot({ task_progress: task({ status: 'completed' }) });
    expect(ferret.update(complete, [], 0).state).toBe('idle');
    ferret.update(snapshot({ task_progress: task() }), [], 1_000);
    expect(ferret.update(complete, [], 2_000).state).toBe('success');
    const project = snapshot().selected_project!;
    expect(ferret.update({ ...complete, selected_project: { ...project, path: 'C:\\other' } }, [], 2_100).state).toBe('idle');
  });
  it('keeps simultaneous error above completion and never repeats terminal reaction', () => {
    const ferret = new FerretController(); ferret.update(snapshot({ task_progress: task() }), [], 1_000);
    const complete = snapshot({ task_progress: task({ status: 'completed' }) });
    const entries = [{ sequence: 1, timestamp_ms: 2_000, level: 'error', source: 'runtime', event_kind: 'failure', message: 'error' }];
    expect(ferret.update(complete, entries, 2_000).state).toBe('error');
    expect(ferret.update(complete, entries, 7_000).state).toBe('idle');
  });
  it('requires exit_code 0 for job success and preserves lost observation as waiting', () => {
    const ferret = new FerretController(); const job = { job_id: 'j1', status: 'running', started_at_ms: 0, finished_at_ms: null, exit_code: null };
    ferret.update(snapshot({ mascot_jobs: [job] }), [], 1_000);
    expect(ferret.update(snapshot({ mascot_jobs: null }), [], 2_000).state).toBe('waiting');
    expect(ferret.update(snapshot({ mascot_jobs: [{ ...job, status: 'completed', finished_at_ms: 2_000 }] }), [], 2_100).state).toBe('waiting');
    expect(new FerretController().update(null, [], 2_100, true, true).state).toBe('error');
  });
});
