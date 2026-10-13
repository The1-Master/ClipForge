export type Appearance = 'light' | 'dark';
/** Semantic colors are shared by the shell, portals, charts and replay. */
export const THEMES = {
  dark: {
    players: '#7AB8FF,#FFB15A,#67DEC0,#FF7B86,#C7A7FF,#67D8E2,#DDD586,#F6AAD1,#BCC8D8,#BDD987',
    background: '#0B0E14', header: '#10151E', panel: '#141A23', raised: '#202A38',
    border: '#344052', text: '#F5F7FC', muted: '#B2BDCF', subtle: '#97A5BB',
    accent: '#FFB15A', accentText: '#241508', accentSoft: 'rgba(255,177,90,.13)',
    success: '#67DEC0', danger: '#FF7B86', teamA: '#7AB8FF', teamB: '#FFB15A',
  },
  light: {
    players: '#2866B5,#A8520B,#14705A,#B92D46,#7753B6,#146B78,#767011,#A33377,#4D617B,#51771A',
    background: '#F3F5F9', header: '#FFFFFF', panel: '#FFFFFF', raised: '#E8EDF5',
    border: '#C5CEDC', text: '#172235', muted: '#4D5D73', subtle: '#596A80',
    accent: '#8F4004', accentText: '#FFFFFF', accentSoft: 'rgba(143,64,4,.09)',
    success: '#14705A', danger: '#B92D46', teamA: '#2866B5', teamB: '#A8520B',
  },
} satisfies Record<Appearance, Record<string, string>>;
export type AppColors = typeof THEMES[Appearance];
export function initialAppearance(saved: string | null, prefersDark: boolean): Appearance {
  return saved === 'dark' || saved === 'light' ? saved : prefersDark ? 'dark' : 'light';
}
