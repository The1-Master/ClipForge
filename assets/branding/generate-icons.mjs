import { readFile, writeFile } from 'node:fs/promises';
const sources = {
  home: 'house-door', clips: 'collection-play', matches: 'layers', activity: 'activity', settings: 'sliders2',
  menu: 'list', play: 'play-fill', pause: 'pause-fill', download: 'download', import: 'file-earmark-plus',
  spark: 'stars', death: 'emoji-dizzy', blooper: 'emoji-laughing', arrowLeft: 'arrow-left', arrowRight: 'arrow-right',
  folder: 'folder2-open', volume: 'volume-up', muted: 'volume-mute', fullscreen: 'fullscreen', search: 'search',
  more: 'three-dots', clock: 'clock', check: 'check2', close: 'x-lg', info: 'info-circle',
  chevronRight: 'chevron-right', chevronDown: 'chevron-down', refresh: 'arrow-clockwise', trash: 'trash3',
  globe: 'globe2', monitor: 'display', sun: 'sun', moon: 'moon-stars', tools: 'tools', storage: 'device-hdd',
  link: 'link-45deg', shield: 'shield-check', stop: 'stop-fill', trophy: 'trophy', export: 'box-arrow-up-right',
  filter: 'funnel', key: 'key', user: 'person', video: 'camera-video', sound: 'soundwave', file: 'file-earmark-play',
  warning: 'exclamation-triangle', eye: 'eye', chart: 'bar-chart', replay: 'skip-backward', copy: 'copy',
};
const icons = {};
for (const [name, file] of Object.entries(sources)) {
  const source = await readFile(new URL(`../../node_modules/bootstrap-icons/icons/${file}.svg`, import.meta.url), 'utf8');
  const elements = [...source.matchAll(/<path\s+([^>]+)\/?\s*>/g)];
  if (!elements.length || source.replace(/<path\s+[^>]+\/?\s*>/g, '').match(/<(?:circle|rect|polygon|ellipse|line)\b/)) throw new Error(`Unsupported SVG ${file}`);
  icons[name] = elements.map(([, attrs]) => {
    const d = attrs.match(/\bd="([^"]+)"/)?.[1];
    if (!d) throw new Error(`Path missing ${file}`);
    return { d, evenodd: /fill-rule="evenodd"/.test(attrs) };
  });
}
const output = `import { useId } from 'react';\n// Bootstrap Icons 1.13.1, MIT. Licensed source paths bundled locally.\n// See assets/branding/Bootstrap-Icons-LICENSE.txt and icon-provenance.json.\nconst icons: Record<string, readonly { d: string; evenodd: boolean }[]> = ${JSON.stringify(icons, null, 2)};\nexport type IconName = ${Object.keys(icons).map(name => JSON.stringify(name)).join(' | ')};\nexport function Icon({ name, size = 20, className = '', title }: { name: string; size?: number; className?: string; title?: string }) {\n  const titleId = useId();\n  return <svg className={\`cf-icon \${className}\`} width={size} height={size} viewBox="0 0 16 16" fill="currentColor" aria-hidden={title ? undefined : true} role={title ? 'img' : undefined} aria-labelledby={title ? titleId : undefined} focusable="false">\n    {title && <title id={titleId}>{title}</title>}\n    {(icons[name] ?? icons.info)!.map((path, index) => <path key={index} d={path.d} fillRule={path.evenodd ? 'evenodd' : 'nonzero'} />)}\n  </svg>;\n}\n`;
await writeFile(new URL('../../src/components/studio/Icon.tsx', import.meta.url), output);
await writeFile(new URL('./icon-provenance.json', import.meta.url), JSON.stringify({ package: 'bootstrap-icons', version: '1.13.1', license: 'MIT', source: 'https://github.com/twbs/icons/tree/v1.13.1/icons', files: sources }, null, 2) + '\n');
