// Line icons drawn on a 24px grid with a 1.6px rounded stroke, close to the
// Fluent System Icons "Regular" weight. Inline SVG instead of the Segoe Fluent
// Icons font so the set is identical on Windows 10 and 11.
const paths = {
  home: 'M4 10.5 12 4l8 6.5V19a1 1 0 0 1-1 1h-4.5v-5.5h-5V20H5a1 1 0 0 1-1-1z',
  folder: 'M3.5 7.5a2 2 0 0 1 2-2h4l2 2h7a2 2 0 0 1 2 2v7.5a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z',
  skills: 'M12 3.5l1.9 4.6 4.6 1.9-4.6 1.9L12 16.5l-1.9-4.6L5.5 10l4.6-1.9zM18.5 15.5l.8 1.7 1.7.8-1.7.8-.8 1.7-.8-1.7-1.7-.8 1.7-.8z',
  link: 'M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1',
  activity: 'M12 7v5l3 2M20.5 12a8.5 8.5 0 1 1-2.5-6',
  settings: 'M12 15.2a3.2 3.2 0 1 0 0-6.4 3.2 3.2 0 0 0 0 6.4zM19 12a7 7 0 0 0-.1-1.2l2-1.5-2-3.4-2.3.9a7 7 0 0 0-2-1.2L14.3 3h-4l-.3 2.6a7 7 0 0 0-2 1.2l-2.4-.9-2 3.4 2 1.5a7 7 0 0 0 0 2.4l-2 1.5 2 3.4 2.4-.9a7 7 0 0 0 2 1.2l.3 2.6h4l.3-2.6a7 7 0 0 0 2-1.2l2.4.9 2-3.4-2-1.5c.1-.4.1-.8.1-1.2z',
  pulse: 'M3 12h4l2.5-6 5 12 2.5-6h4',
  refresh: 'M19.5 8.5A8 8 0 1 0 20 12M20 4v4.5h-4.5',
  laptop: 'M5 6.5a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1V15H5zM3 18h18',
  shield: 'M12 3.5 19 6v5.5c0 4.2-3 7.6-7 9-4-1.4-7-4.8-7-9V6z',
  spark: 'M12 3l2.2 6.8L21 12l-6.8 2.2L12 21l-2.2-6.8L3 12l6.8-2.2z',
  check: 'M5 12.5l4.5 4.5L19 7.5',
  play: 'M8 5.5v13l10-6.5z',
  warning: 'M12 4 21 19.5H3zM12 10v4.5M12 17.2v.1',
  error: 'M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM9 9l6 6M15 9l-6 6',
  gear: 'M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z',
  dot: 'M12 14a2 2 0 1 0 0-4 2 2 0 0 0 0 4z',
  external: 'M14 4.5h5.5V10M19.5 4.5 11 13M17 14v4.5a1 1 0 0 1-1 1H5.5a1 1 0 0 1-1-1V8a1 1 0 0 1 1-1H10',
  copy: 'M8.5 8.5V5.5a1 1 0 0 1 1-1h9a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1h-3M5.5 8.5h9a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1v-9a1 1 0 0 1 1-1z',
  chevron: 'M9 5.5 15.5 12 9 18.5',
} as const;

export type IconName = keyof typeof paths;

export function Icon({ name, size = 20, className = '' }: { name: IconName; size?: number; className?: string }) {
  return <svg className={`icon ${className}`} width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor"
    strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false"><path d={paths[name]} /></svg>;
}
