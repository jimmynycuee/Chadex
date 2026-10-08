import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen, within } from '@testing-library/react';
import { ActivityTimeline } from '../src/ActivityTimeline';
import type { RuntimeActivityEntry } from '../src/contracts';

afterEach(cleanup);
const now = new Date(2026, 9, 9, 15, 0).getTime();
const entry = (sequence: number, timestamp_ms: number, level = 'info', event_kind = 'operation_started'): RuntimeActivityEntry =>
  ({ sequence, timestamp_ms, source: 'chadex', level, event_kind, message: `event ${sequence}` });

describe('ActivityTimeline', () => {
  it('groups newest-first entries under today, yesterday and dated headers', () => {
    render(<ActivityTimeline now={now} entries={[
      entry(3, now - 60_000), entry(2, now - 26 * 3_600_000), entry(1, now - 50 * 3_600_000),
    ]} />);
    expect(screen.getAllByRole('heading', { level: 3 }).map((h) => h.textContent)).toEqual(['今天', '昨天', expect.stringMatching(/10月7日/)]);
  });
  it('speaks warning and error levels instead of relying on the node color', () => {
    render(<ActivityTimeline now={now} entries={[entry(2, now, 'error', 'tunnel_disconnected'), entry(1, now, 'warning')]} />);
    const rows = screen.getAllByRole('listitem');
    expect(within(rows[0]).getByText(/錯誤：/)).toBeTruthy();
    expect(within(rows[1]).getByText(/警告：/)).toBeTruthy();
  });
});
