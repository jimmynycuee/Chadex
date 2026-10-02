import type { MascotJob, McpPerformanceTrace } from './contracts';
import { safeError } from './state';

function elapsed(trace: McpPerformanceTrace) {
  if (trace.total_us != null && Number.isFinite(trace.total_us)) return `${Math.max(0, Math.round(trace.total_us / 1_000)).toLocaleString('zh-TW')} ms`;
  if (trace.finished_at_ms != null) return `${Math.max(0, trace.finished_at_ms - trace.started_at_ms).toLocaleString('zh-TW')} ms`;
  return '尚未結束';
}
export function JobActivity({ jobs, available }: { jobs: MascotJob[]; available: boolean }) {
  return jobs.length ? <div className="activity-table-wrap"><table className="activity-table"><caption className="sr-only">本地 Job 狀態</caption><thead><tr><th scope="col">Job ID</th><th scope="col">狀態</th><th scope="col">Exit code</th></tr></thead><tbody>{jobs.map((job) => <tr key={job.job_id}><td><code>{job.job_id}</code></td><td>{job.status}</td><td>{job.exit_code ?? '尚未回報'}</td></tr>)}</tbody></table></div>
    : <p className="panel-note">{available ? '目前沒有可觀測的 Job。' : 'Job 狀態未確認。'}</p>;
}
export function TraceActivity({ traces, available }: { traces: McpPerformanceTrace[]; available: boolean }) {
  return traces.length ? <div className="activity-table-wrap"><table className="activity-table"><caption className="sr-only">真實工具與命令呼叫</caption><thead><tr><th scope="col">工具</th><th scope="col">完成狀態</th><th scope="col">HTTP 狀態</th><th scope="col">耗時</th></tr></thead><tbody>{traces.slice().sort((a, b) => b.sequence - a.sequence).map((trace) => <tr key={trace.sequence}>
    <td>{trace.tool_names.length ? trace.tool_names.map((name, index) => <code className="trace-tool" key={`${index}:${name}`}>{name}</code>) : '未回報工具名稱'}<small>#{trace.sequence}</small></td>
    <td><span className={trace.tool_failed === true || (trace.status_code ?? 0) >= 400 ? 'trace-failure' : ''}>{safeError(trace.completion ?? (trace.finished_at_ms == null ? '進行中' : '未回報'))}</span>{trace.tool_failed === true && <small>工具回報失敗</small>}</td>
    <td>{trace.status_code ?? '未回報'}</td><td>{elapsed(trace)}</td>
  </tr>)}</tbody></table></div> : <p className="panel-note">{available ? '目前沒有可觀測的工具呼叫紀錄。' : '工具呼叫狀態未確認。'}</p>;
}
