import canonical from 'virtual:ferret-canonical';
import type { BackendSnapshot, RuntimeActivityEntry, TaskProgress } from './contracts';

export { canonical };
export interface FerretPresentation { state: string; activity: string | null; progress: number | null; recent: boolean; since: number }
export function toolState(name: string) { return canonical.toolStates[name] ?? 'thinking'; }
export function taskState(task: TaskProgress) {
  if (task.status === 'queued' || task.status === 'cancelling' || task.cancel_requested) return 'waiting';
  if (task.status === 'validating') return 'testing';
  if (task.status === 'integrating') return 'coding';
  return canonical.taskSteps[task.plan[task.current_step]] ?? 'thinking';
}

// Projection of facts that desktop_state actually exposes. Traces are absent
// from this contract: never infer code/test activity from an untyped log string.
export class FerretController {
  private path: string | null | undefined;
  private taskId: string | undefined;
  private taskStatus: string | undefined;
  private sequence = 0;
  private terminalJobs = new Set<string>();
  private lastActiveJobs = false;
  private lastActivityAt = 0;
  private busyId: string | undefined;
  private receivedAt = -Infinity;
  private reaction: { state: string; at: number; until: number } | null = null;
  private presentation: FerretPresentation = { state: 'idle', activity: null, progress: null, recent: false, since: 0 };
  private react(state: string, now: number) {
    if (this.reaction?.at === now && this.reaction.state === 'error' && state !== 'error') return;
    this.reaction = { state, at: now, until: now + (state === 'error' ? 5_000 : 3_000) };
  }
  update(snapshot: BackendSnapshot | null, activities: RuntimeActivityEntry[], now: number, foreground = true, unavailable = false): FerretPresentation {
    if (!snapshot) {
      this.path = undefined; this.reaction = null; this.busyId = undefined;
      return this.present(unavailable ? 'error' : 'waiting', null, null, now);
    }
    const path = snapshot.selected_project?.path ?? null;
    const task = snapshot.task_progress;
    const jobs = snapshot.mascot_jobs;
    if (this.path === undefined || path !== this.path) {
      this.path = path; this.taskId = task?.task_id; this.taskStatus = task?.status;
      this.sequence = Math.max(0, ...activities.map((entry) => entry.sequence));
      this.terminalJobs = new Set((jobs ?? []).filter((job) => !canonical.activeJobs.includes(job.status)).map((job) => job.job_id));
      this.lastActiveJobs = false; this.lastActivityAt = now; this.busyId = undefined; this.reaction = null;
    }
    const activeTask = Boolean(task && canonical.activeTasks.includes(task.status));
    const activeJobs = (jobs ?? []).filter((job) => canonical.activeJobs.includes(job.status));
    const age = (time: number) => Math.max(0, now - time);
    const starts = [...activeJobs.filter((job) => !canonical.waitingJobs.includes(job.status)).flatMap((job) => job.started_at_ms === null ? [] : [job.started_at_ms]),
      ...(task && activeTask ? [task.started_at_ms] : [])];
    if (this.reaction && (now >= this.reaction.until || starts.some((start) => start > this.reaction!.at))) this.reaction = null;
    const newEvents = activities.filter((entry) => entry.sequence > this.sequence);
    if (newEvents.length) {
      this.sequence = Math.max(...newEvents.map((entry) => entry.sequence)); this.lastActivityAt = now;
      if (newEvents.some((entry) => entry.level === 'error')) this.react('error', now);
    }
    if (task && (task.task_id !== this.taskId || task.status !== this.taskStatus)) {
      this.taskId = task.task_id; this.taskStatus = task.status; this.lastActivityAt = now;
      if (task.status === 'completed' && task.validation.status !== 'failed') this.react('success', now);
      else if (['failed', 'failed_validation'].includes(task.status) || task.validation.status === 'failed') this.react('error', now);
    }
    if (jobs) {
      for (const job of jobs.filter((job) => !canonical.activeJobs.includes(job.status) && !this.terminalJobs.has(job.job_id))) {
        this.terminalJobs.add(job.job_id);
        if (job.finished_at_ms !== null && age(job.finished_at_ms) < 8_000) {
          this.lastActivityAt = now;
          if (['failed', 'lost', 'timeout', 'timed_out', 'process_lost'].includes(job.status) || (job.exit_code !== null && job.exit_code !== 0)) this.react('error', now);
          else this.react(['completed', 'exited'].includes(job.status) && job.exit_code === 0 ? 'success' : 'waiting', now);
        }
      }
      this.terminalJobs = new Set([...this.terminalJobs].filter((id) => jobs.some((job) => job.job_id === id)));
      this.lastActiveJobs = activeJobs.length > 0;
    }
    const busyId = task && activeTask ? `task:${task.task_id}` : activeJobs[0] ? `job:${activeJobs[0].job_id}` : undefined;
    if (busyId && busyId !== this.busyId) {
      const start = task && activeTask ? task.started_at_ms : activeJobs[0]?.started_at_ms;
      this.receivedAt = start != null && age(start) < 8_000 ? now : -Infinity;
      this.lastActivityAt = now;
    }
    this.busyId = busyId;
    let state = 'idle'; let activity: string | null = null; let progress: number | null = null;
    if (task && activeTask) {
      state = activity = taskState(task);
      if (task.total_steps > 0) progress = Math.min(1, Math.max(0, task.completed_steps / task.total_steps));
      if (state !== 'waiting' && age(task.started_at_ms) >= 60_000) state = 'longTask';
    } else if (activeJobs.length) {
      state = activity = activeJobs.every((job) => canonical.waitingJobs.includes(job.status)) ? 'waiting' : 'thinking';
      const starts = activeJobs.flatMap((job) => job.started_at_ms === null ? [] : [job.started_at_ms]);
      if (state !== 'waiting' && starts.length && age(Math.min(...starts)) >= 60_000) state = 'longTask';
    } else if (snapshot.error) state = 'error';
    else if (task && ['blocked', 'interrupted', 'ready_to_apply', 'unknown'].includes(task.status)) state = 'waiting';
    else if ((jobs ?? []).some((job) => job.status === 'unknown') || (jobs == null && this.lastActiveJobs)) state = 'waiting';
    else if (snapshot.current_operation) {
      state = snapshot.current_operation.phase === 'cancelling' ? 'waiting' : age(snapshot.current_operation.started_at_ms) >= 60_000 ? 'longTask' : 'thinking';
    } else if (this.reaction && now < this.reaction.until) state = this.reaction.state;
    else if (now - this.lastActivityAt >= 180_000 || !foreground) state = 'sleep';
    if (['thinking', 'searching', 'coding', 'testing', 'longTask'].includes(state) && busyId && now - this.receivedAt < 1_000) state = 'listening';
    return this.present(state, activity, progress, now);
  }
  private present(state: string, activity: string | null, progress: number | null, now: number) {
    this.presentation = { state, activity, progress, recent: false, since: state === this.presentation.state ? this.presentation.since : now };
    return this.presentation;
  }
}
