import type { ReactElement, SVGProps } from 'react';

export type IconName =
  | 'arrow-left' | 'arrow-right' | 'check' | 'chevron-down' | 'chevron-right'
  | 'clock' | 'list' | 'download' | 'file' | 'folder' | 'forward-10' | 'grid' | 'headphones' | 'image'
  | 'mic' | 'more' | 'pause' | 'play' | 'plus' | 'refresh' | 'rewind-10' | 'search' | 'settings'
  | 'spark' | 'trash' | 'upload' | 'video' | 'volume' | 'volume-mute' | 'waveform' | 'x';

const paths: Record<IconName, ReactElement> = {
  'arrow-left': <><path d="m15 18-6-6 6-6" /><path d="M9 12h10" /></>,
  'arrow-right': <><path d="M5 12h10" /><path d="m13 6 6 6-6 6" /></>,
  check: <path d="m5 12 4 4L19 6" />,
  'chevron-down': <path d="m6 9 6 6 6-6" />,
  'chevron-right': <path d="m9 18 6-6-6-6" />,
  clock: <><circle cx="12" cy="12" r="8" /><path d="M12 8v4l3 2" /></>,
  list: <><path d="M8 6h13M8 12h13M8 18h13" /><circle cx="4" cy="6" r="0.5" fill="currentColor" /><circle cx="4" cy="12" r="0.5" fill="currentColor" /><circle cx="4" cy="18" r="0.5" fill="currentColor" /></>,
  download: <><path d="M12 4v10" /><path d="m8 10 4 4 4-4" /><path d="M5 19h14" /></>,
  file: <><path d="M6 3h8l4 4v14H6z" /><path d="M14 3v5h5" /></>,
  'forward-10': <><path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8" /><path d="M21 3v5h-5" /><text x="12" y="16.6" textAnchor="middle" fontSize="8" fontWeight="700" fill="currentColor" stroke="none">10</text></>,
  'rewind-10': <><path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" /><path d="M3 3v5h5" /><text x="12" y="16.6" textAnchor="middle" fontSize="8" fontWeight="700" fill="currentColor" stroke="none">10</text></>,
  folder: <path d="M3 6h6l2 2h10v10H3z" />,
  grid: <><rect x="4" y="4" width="6" height="6" /><rect x="14" y="4" width="6" height="6" /><rect x="4" y="14" width="6" height="6" /><rect x="14" y="14" width="6" height="6" /></>,
  headphones: <><path d="M4 15v-3a8 8 0 0 1 16 0v3" /><path d="M4 14h3v6H5a1 1 0 0 1-1-1z" /><path d="M20 14h-3v6h2a1 1 0 0 0 1-1z" /></>,
  image: <><rect x="4" y="4" width="16" height="16" rx="2" /><circle cx="9" cy="9" r="1.5" /><path d="m5 17 4-4 3 3 2-2 5 5" /></>,
  mic: <><rect x="9" y="3" width="6" height="11" rx="3" /><path d="M5 11a7 7 0 0 0 14 0M12 18v3M9 21h6" /></>,
  more: <><circle cx="5" cy="12" r="1" fill="currentColor" /><circle cx="12" cy="12" r="1" fill="currentColor" /><circle cx="19" cy="12" r="1" fill="currentColor" /></>,
  pause: <><path d="M8 5v14M16 5v14" /></>,
  play: <path d="m8 5 11 7-11 7z" fill="currentColor" stroke="none" />,
  plus: <><path d="M12 5v14M5 12h14" /></>,
  refresh: <><path d="M20 11a8 8 0 0 0-14.7-3L4 10" /><path d="M4 5v5h5" /><path d="M4 13a8 8 0 0 0 14.7 3L20 14" /><path d="M20 19v-5h-5" /></>,
  search: <><circle cx="10.5" cy="10.5" r="6" /><path d="m16 16 4 4" /></>,
  settings: <><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1-1.4 1.4-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.6v.2h-2v-.2a1.7 1.7 0 0 0-1-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1L9 17l.1-.1a1.7 1.7 0 0 0 .3-1.9 1.7 1.7 0 0 0-1.6-1H7v-2h.2a1.7 1.7 0 0 0 1.6-1 1.7 1.7 0 0 0-.3-1.9L8.4 9 9.8 7.6l.1.1a1.7 1.7 0 0 0 1.9.3 1.7 1.7 0 0 0 1-1.6v-.2h2v.2a1.7 1.7 0 0 0 1 1.6 1.7 1.7 0 0 0 1.9-.3l.1-.1L19.8 9l-.1.1a1.7 1.7 0 0 0-.3 1.9 1.7 1.7 0 0 0 1.6 1h.2v2H21a1.7 1.7 0 0 0-1.6 1z" /></>,
  spark: <><path d="m12 3 1.4 5.6L19 10l-5.6 1.4L12 17l-1.4-5.6L5 10l5.6-1.4z" /><path d="m19 16 .6 2.4L22 19l-2.4.6L19 22l-.6-2.4L16 19l2.4-.6z" /></>,
  trash: <><path d="M5 7h14M10 11v6M14 11v6" /><path d="M8 7l1-3h6l1 3M7 7l1 14h8l1-14" /></>,
  upload: <><path d="M12 16V5" /><path d="m8 9 4-4 4 4" /><path d="M5 19h14" /></>,
  video: <><rect x="3" y="6" width="13" height="12" rx="2" /><path d="m16 10 5-3v10l-5-3z" /></>,
  volume: <><path d="M11 5 6 9H3v6h3l5 4z" /><path d="M15.5 8.5a5 5 0 0 1 0 7" /><path d="M18.6 5.6a9 9 0 0 1 0 12.8" /></>,
  'volume-mute': <><path d="M11 5 6 9H3v6h3l5 4z" /><path d="m16 9 5 5" /><path d="m21 9-5 5" /></>,
  waveform: <path d="M3 12h2m2-4v8m3-11v14m4-9v6m4-10v14m3-8h2" />,
  x: <><path d="m6 6 12 12M18 6 6 18" /></>,
};

export default function Icon({ name, size = 16, strokeWidth = 1.7, ...props }: { name: IconName; size?: number; strokeWidth?: number } & SVGProps<SVGSVGElement>) {
  return <svg {...props} width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={strokeWidth} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{paths[name]}</svg>;
}
