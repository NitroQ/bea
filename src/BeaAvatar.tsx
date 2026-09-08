// Bea — pixel-art avatar rendered as crisp SVG rects on a 48x56 grid.
// No image assets: the logo and every variant ship as code, so they stay
// sharp at any size and work offline in the Tauri bundle.
export type BeaVariant =
  | 'logo'
  | 'presenting'
  | 'waving'
  | 'happy'
  | 'love'
  | 'curious'
  | 'celebrating';

export const BEA_GRID_WIDTH = 48;
export const BEA_GRID_HEIGHT = 56;

const PALETTE = {
  hair: '#1b1522',
  hairHi: '#3d3452',
  skin: '#f6bda6',
  skinSh: '#df9077',
  blush: '#f2a0a0',
  white: '#f4f6fb',
  pupil: '#26232e',
  mouth: '#9c4040',
  mouthOpen: '#5f2222',
  teeth: '#ffffff',
  tongue: '#e46969',
  dress: '#d92e2e',
  dressDk: '#9e1c1c',
  dressLt: '#f26a6a',
  collar: '#f6f1e8',
  shoe: '#2b2433',
  amber: '#e0b774',
  teal: '#84e2c5',
  pink: '#f2a0c0',
  cloud: '#dfe7f2',
} as const;

type PaletteKey = keyof typeof PALETTE;
// [x, y, width, height, color] — all units are grid pixels.
type PixelRect = [x: number, y: number, w: number, h: number, color: PaletteKey];

// Shared body: black bob with a red bow, pinkish skin, red dress.
// Big face, straight bangs, no brow notches (they read as anger).
const BASE: PixelRect[] = [
  // Back hair + shine.
  [10, 4, 28, 8, 'hair'],
  [6, 12, 4, 24, 'hair'],
  [38, 12, 4, 22, 'hair'],
  [12, 4, 8, 2, 'hairHi'],
  [6, 12, 2, 8, 'hairHi'],
  // Face + neck.
  [10, 12, 28, 20, 'skin'],
  [22, 32, 4, 2, 'skinSh'],
  // Straight-across bangs.
  [10, 12, 28, 4, 'hair'],
  // Legs + shoes.
  [18, 48, 4, 4, 'skin'],
  [26, 48, 4, 4, 'skin'],
  [18, 52, 4, 2, 'shoe'],
  [26, 52, 4, 2, 'shoe'],
  // Red dress: bodice, collar, belt, skirt with folds.
  [14, 34, 20, 6, 'dress'],
  [20, 34, 2, 2, 'collar'],
  [26, 34, 2, 2, 'collar'],
  [14, 40, 20, 2, 'dressDk'],
  [10, 42, 28, 6, 'dress'],
  [16, 42, 2, 6, 'dressDk'],
  [30, 42, 2, 6, 'dressDk'],
  [22, 42, 4, 2, 'dressLt'],
  // Red bow, top-left (drawn last so it sits on the hair).
  [2, 0, 6, 4, 'dress'],
  [10, 0, 6, 4, 'dress'],
  [6, 0, 4, 6, 'dressDk'],
  [2, 0, 6, 2, 'dressLt'],
];

// Big shiny dark eyes with mirrored sparkles — the friendly default face.
const SHINY_EYES: PixelRect[] = [
  [14, 18, 4, 6, 'pupil'],
  [30, 18, 4, 6, 'pupil'],
  [14, 18, 2, 2, 'white'],
  [32, 18, 2, 2, 'white'],
];
const SOFT_BLUSH: PixelRect[] = [
  [12, 24, 4, 2, 'blush'],
  [32, 24, 4, 2, 'blush'],
];
// Gentle curved ∪ smile.
const SOFT_SMILE: PixelRect[] = [
  [20, 26, 2, 2, 'mouth'],
  [22, 28, 4, 2, 'mouth'],
  [26, 26, 2, 2, 'mouth'],
];

const VARIANTS: Record<BeaVariant, { label: string; rects: PixelRect[] }> = {
  // The app logo: confident, arms crossed, gentle smile.
  logo: {
    label: 'Bea · logo',
    rects: [
      ...SHINY_EYES,
      ...SOFT_BLUSH,
      ...SOFT_SMILE,
      [12, 34, 6, 2, 'dress'],
      [30, 34, 6, 2, 'dress'],
      [18, 38, 12, 2, 'skin'],
      [20, 40, 8, 2, 'skinSh'],
    ],
  },
  // One arm extended, presenting the meeting.
  presenting: {
    label: 'Bea presenting',
    rects: [
      ...SHINY_EYES,
      ...SOFT_BLUSH,
      // Open welcoming smile.
      [20, 26, 8, 2, 'teeth'],
      [20, 28, 8, 2, 'mouthOpen'],
      [12, 34, 6, 2, 'dress'],
      [10, 36, 4, 6, 'skin'],
      [30, 34, 6, 2, 'dress'],
      [36, 36, 10, 4, 'skin'],
    ],
  },
  // Waving hello — "here whenever you need a hand."
  waving: {
    label: 'Bea waving',
    rects: [
      ...SHINY_EYES,
      ...SOFT_BLUSH,
      ...SOFT_SMILE,
      // Left arm resting.
      [12, 34, 6, 2, 'dress'],
      [10, 36, 4, 6, 'skin'],
      // Right arm rising diagonally from the shoulder (stepped pixels).
      [34, 32, 4, 4, 'skin'],
      [36, 28, 4, 4, 'skin'],
      [38, 24, 4, 4, 'skin'],
      [40, 20, 4, 4, 'skin'],
      // Short sleeve at the shoulder.
      [32, 32, 5, 3, 'dress'],
      // Open palm with fingers.
      [40, 16, 5, 4, 'skin'],
      [40, 14, 2, 2, 'skin'],
      [43, 14, 2, 2, 'skin'],
      // Wave motion ticks.
      [46, 13, 1, 2, 'amber'],
      [46, 17, 1, 2, 'amber'],
    ],
  },
  // Closed happy eyes, blush, toothy smile.
  happy: {
    label: 'Bea happy',
    rects: [
      [14, 20, 2, 2, 'pupil'],
      [16, 18, 2, 2, 'pupil'],
      [18, 20, 2, 2, 'pupil'],
      [28, 20, 2, 2, 'pupil'],
      [30, 18, 2, 2, 'pupil'],
      [32, 20, 2, 2, 'pupil'],
      [12, 24, 6, 2, 'blush'],
      [30, 24, 6, 2, 'blush'],
      [20, 26, 8, 2, 'teeth'],
      [22, 28, 4, 2, 'mouthOpen'],
      [12, 34, 6, 2, 'dress'],
      [30, 34, 6, 2, 'dress'],
      [10, 36, 4, 6, 'skin'],
      [34, 36, 4, 6, 'skin'],
    ],
  },
  // Heart eyes, big happy smile, arms thrown up — pure "love it!"
  love: {
    label: 'Bea love',
    rects: [
      [13, 17, 2, 1, 'dress'],
      [16, 17, 2, 1, 'dress'],
      [13, 18, 5, 1, 'dress'],
      [13, 19, 5, 1, 'dress'],
      [14, 20, 3, 1, 'dress'],
      [15, 21, 1, 1, 'dress'],
      [30, 17, 2, 1, 'dress'],
      [33, 17, 2, 1, 'dress'],
      [30, 18, 5, 1, 'dress'],
      [30, 19, 5, 1, 'dress'],
      [31, 20, 3, 1, 'dress'],
      [32, 21, 1, 1, 'dress'],
      [13, 18, 1, 1, 'white'],
      [30, 18, 1, 1, 'white'],
      [12, 24, 6, 2, 'blush'],
      [30, 24, 6, 2, 'blush'],
      [18, 26, 12, 2, 'teeth'],
      [18, 28, 12, 2, 'mouthOpen'],
      [12, 34, 4, 2, 'dress'],
      [32, 34, 4, 2, 'dress'],
      [4, 22, 4, 14, 'skin'],
      [4, 20, 4, 2, 'skin'],
      [42, 22, 4, 14, 'skin'],
      [42, 20, 4, 2, 'skin'],
    ],
  },
  // Gazing up at a thought cloud — for empty states and no-result searches.
  curious: {
    label: 'Bea curious',
    rects: [
      // Same friendly eyes as presenting.
      ...SHINY_EYES,
      ...SOFT_BLUSH,
      [21, 27, 6, 2, 'mouth'],
      // Arms resting.
      [12, 34, 6, 2, 'dress'],
      [30, 34, 6, 2, 'dress'],
      [10, 36, 4, 6, 'skin'],
      [34, 36, 4, 6, 'skin'],
      // Thought cloud with a question hook, balanced opposite the bow.
      [33, 0, 7, 3, 'cloud'],
      [30, 2, 13, 5, 'cloud'],
      [28, 8, 2, 2, 'cloud'],
      [25, 11, 2, 2, 'cloud'],
      [34, 3, 5, 1, 'amber'],
      [38, 4, 1, 1, 'amber'],
      [37, 5, 2, 1, 'amber'],
    ],
  },
  // Arms high with confetti — something worth celebrating.
  celebrating: {
    label: 'Bea celebrating',
    rects: [
      [14, 20, 2, 2, 'pupil'],
      [16, 18, 2, 2, 'pupil'],
      [18, 20, 2, 2, 'pupil'],
      [28, 20, 2, 2, 'pupil'],
      [30, 18, 2, 2, 'pupil'],
      [32, 20, 2, 2, 'pupil'],
      [12, 24, 6, 2, 'blush'],
      [30, 24, 6, 2, 'blush'],
      [18, 26, 12, 2, 'teeth'],
      [18, 28, 12, 2, 'mouthOpen'],
      [12, 34, 4, 2, 'dress'],
      [32, 34, 4, 2, 'dress'],
      [2, 18, 4, 16, 'skin'],
      [2, 16, 4, 2, 'skin'],
      [42, 18, 4, 16, 'skin'],
      [42, 16, 4, 2, 'skin'],
      [0, 4, 2, 2, 'teal'],
      [20, 0, 2, 2, 'amber'],
      [32, 0, 2, 2, 'pink'],
      [46, 8, 2, 2, 'white'],
      [0, 28, 2, 2, 'amber'],
      [46, 32, 2, 2, 'teal'],
    ],
  },
};

export const BEA_VARIANTS = Object.keys(VARIANTS) as BeaVariant[];

export function beaLabel(variant: BeaVariant): string {
  return VARIANTS[variant].label;
}

export function beaRects(variant: BeaVariant): PixelRect[] {
  return [...BASE, ...VARIANTS[variant].rects];
}

export default function BeaAvatar({
  variant = 'logo',
  size = 40,
  className = '',
  title,
}: {
  variant?: BeaVariant;
  size?: number;
  className?: string;
  title?: string;
}) {
  const pixels = beaRects(variant);
  return (
    <svg
      className={`bea-avatar${className ? ` ${className}` : ''}`}
      width={size}
      height={Math.round((size * BEA_GRID_HEIGHT) / BEA_GRID_WIDTH)}
      viewBox={`0 0 ${BEA_GRID_WIDTH} ${BEA_GRID_HEIGHT}`}
      shapeRendering="crispEdges"
      data-variant={variant}
      role="img"
      aria-label={title ?? VARIANTS[variant].label}
    >
      {pixels.map(([x, y, w, h, color], index) => (
        <rect key={index} x={x} y={y} width={w} height={h} fill={PALETTE[color]} />
      ))}
    </svg>
  );
}
