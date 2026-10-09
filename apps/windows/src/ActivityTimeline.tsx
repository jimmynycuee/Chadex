import type { RuntimeActivityEntry } from './contracts';
import { Icon, type IconName } from './icons';
import { safeError } from './state';

type Tone = 'signal' | 'good' | 'warn' | 'danger';

function startOfDay(time: number) { const day = new Date(time); day.setHours(0, 0, 0, 0); return day.getTime(); }
function dayLabel(day: number, now: number) {
  const today = startOfDay(now);
  if (day === today) return '今天';
  if (day === today - 86_400_000) return '昨天';
  return new Date(day).toLocaleDateString('zh-TW', { month: 'long', day: 'numeric', weekday: 'long' });
}
const time = (value: number) => new Date(value).toLocaleTimeString('zh-TW', { hour: '2-digit', minute: '2-digit', hour12: false });
const levelText: Record<string, string> = { warning: '警告', error: '錯誤' };

/** What happened, not only how severe it was: the symbol and the spoken level carry it without color. */
function presentation(entry: RuntimeActivityEntry): { icon: IconName; tone: Tone } {
  if (entry.level === 'error') return { icon: 'error', tone: 'danger' };
  if (entry.level === 'warning') return { icon: 'warning', tone: 'warn' };
  const kind = entry.event_kind;
  if (kind === 'local_runtime_ready') return { icon: 'check', tone: 'good' };
  if (kind === 'project_activated') return { icon: 'folder', tone: 'signal' };
  if (kind === 'operation_started') return { icon: 'play', tone: 'signal' };
  if (kind === 'process_started') return { icon: 'gear', tone: 'signal' };
  if (kind.includes('tunnel') || kind.includes('connect')) return { icon: 'link', tone: 'signal' };
  return { icon: 'dot', tone: 'signal' };
}

/**
 * Activity as a day timeline: time on the left, one colored node per event on a
 * continuous spine, the message on the right. Entries arrive newest first.
 */
export function ActivityTimeline({ entries, showMeta = false, now = Date.now() }: { entries: RuntimeActivityEntry[]; showMeta?: boolean; now?: number }) {
  const days: { day: number; entries: RuntimeActivityEntry[] }[] = [];
  for (const entry of entries) {
    const day = startOfDay(entry.timestamp_ms);
    const last = days[days.length - 1];
    if (last && last.day === day) last.entries.push(entry); else days.push({ day, entries: [entry] });
  }
  return <div className="timeline">{days.map((group, index) => <section key={`${group.day}-${index}`} className="timeline-day">
    <h3>{dayLabel(group.day, now)}</h3>
    <ol>{group.entries.map((entry) => {
      const { icon, tone } = presentation(entry);
      const level = levelText[entry.level];
      return <li key={entry.sequence} className="timeline-row" title={`${entry.source} · ${entry.event_kind} · #${entry.sequence}`}>
        <time dateTime={new Date(entry.timestamp_ms).toISOString()}>{time(entry.timestamp_ms)}</time>
        <span className={`timeline-node ${tone}`}><Icon name={icon} size={14} /></span>
        <div className="timeline-body">
          <p>{level && <span className="sr-only">{level}：</span>}{safeError(entry.message)}</p>
          {showMeta && <small>{entry.source} · {entry.event_kind}</small>}
        </div>
      </li>;
    })}</ol>
  </section>)}</div>;
}
