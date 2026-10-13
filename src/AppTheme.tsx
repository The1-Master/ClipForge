import { createContext, useContext, useEffect, useState, type ReactNode } from 'react';
import { Theme } from '@radix-ui/themes';
import { THEMES, initialAppearance, type Appearance } from './themes.ts';
// Portals create sibling Theme nodes; local inline variables do not reach them.
const themeCss = Object.entries(THEMES).map(([appearance, colors]) =>
  `.radix-themes.${appearance}{${Object.entries(colors).map(([key, value]) => `--app-${key}:${value}`).join(';')}}`,
).join('\n');
// Existing saved keys retain their body size: standard, comfortable and large.
export const FONT_SIZES = { small: [30, 20, 16, 13], medium: [32, 22, 18, 14], large: [36, 24, 20, 15] } as const;
export type FontSize = keyof typeof FONT_SIZES;
const ThemeContext = createContext({ appearance: 'dark' as Appearance, colors: THEMES.dark, toggle: () => {}, fontSize: 'small' as FontSize, setFontSize: (_size: FontSize) => {}, typography: FONT_SIZES.small as readonly number[] });
export const useAppTheme = () => useContext(ThemeContext);
export function AppTheme({ children }: { children: ReactNode }) {
  const [themeChanging, setThemeChanging] = useState(false);
  const [appearance, setAppearance] = useState<Appearance>(() => {
    let saved: string | null = null;
    try { saved = localStorage.getItem('demodesk.appearance'); } catch { /* WebView storage may be unavailable. */ }
    return initialAppearance(saved, matchMedia('(prefers-color-scheme: dark)').matches);
  });
  const [fontSize, setFontSize] = useState<FontSize>(() => {
    try { const saved = localStorage.getItem('demodesk.fontSize'); if (saved === 'small' || saved === 'medium' || saved === 'large') return saved; } catch { /* Storage is optional. */ }
    return 'small';
  });
  useEffect(() => { try { localStorage.setItem('demodesk.fontSize', fontSize); } catch { /* Storage is optional. */ } }, [fontSize]);
  const typography = FONT_SIZES[fontSize];
  const fontCss = '.radix-themes{' + ['title', 'subtitle', 'body', 'caption'].map((role, i) => `--app-font-${role}:${typography[i]}px`).join(';') + `;--cf-speed:${themeChanging ? '0ms' : '180ms'}}`;
  const colors = THEMES[appearance];
  useEffect(() => {
    try { localStorage.setItem('demodesk.appearance', appearance); } catch { /* Theme changes still work without persistence. */ }
  }, [appearance]);
  // Apply palette changes together; interpolated foregrounds can lose contrast.
  useEffect(() => {
    if (!themeChanging) return;
    const timer = setTimeout(() => setThemeChanging(false), 220);
    return () => clearTimeout(timer);
  }, [themeChanging]);
  const toggle = () => { setThemeChanging(true); setAppearance(current => current === 'dark' ? 'light' : 'dark'); };
  return <ThemeContext.Provider value={{ appearance, colors, toggle, fontSize, setFontSize, typography }}><Theme panelBackground="solid" appearance={appearance} accentColor="amber" grayColor="slate" radius="large" scaling="100%"><style>{themeCss}{fontCss}</style><div className="app-root">{children}</div></Theme></ThemeContext.Provider>;
}
