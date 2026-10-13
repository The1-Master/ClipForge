import assert from 'node:assert/strict';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { Theme } from '@radix-ui/themes';
import { Provider as TooltipProvider } from '@radix-ui/react-tooltip';
import { createServer } from 'vite';
const server = await createServer({ server: { middlewareMode: true }, appType: 'custom' });
try {
  const { default: i18n } = await import('i18next');
  const { initReactI18next } = await import('react-i18next');
  await i18n.use(initReactI18next).init({ lng: 'en', resources: { en: { translation: {} } }, initImmediate: false });
  const { TeamPanel } = await server.ssrLoadModule('/src/components/replay/TeamPanel.tsx');
  const player = { pid:1, name:'NOVA', alive:true, hp:120, armor:100, helmet:true, defuser:true, bomb:false, money:4200, weapon:'AK-47', inventory:['AK-47','USP-S','Flashbang','Flashbang'], kills:24, deaths:15, assists:4 };
  const render = p => renderToStaticMarkup(createElement(TooltipProvider, null,
    createElement(Theme, null, createElement(TeamPanel, {side:'CT',label:'Team A',score:8,players:[p],focus:1,onFocus:()=>{}}))));
  const alive = render(player);
  assert.match(alive,/--health:100%/);
  assert.match(alive,/24\/15\/4/);
  assert.match(alive,/alt="Defuse Kit"/);
  assert.match(alive,/alt="AK-47"/);
  assert.match(alive,/alt="USP-S"/);
  assert.equal((alive.match(/alt="Flashbang"/g) || []).length,2, 'Inventory retains both flashbangs');
  assert.match(alive,/aria-pressed="true"/);
  assert.match(render({...player,hp:17}),/--health:17%/);
  assert.match(render({...player,hp:17}),/17 HP/);
  const dead = render({...player,alive:false,hp:80});
  assert.match(dead,/--health:0%/);
  assert.match(dead,/24\/15\/4/);
  assert.doesNotMatch(dead,/alt="Defuse Kit"/);
  assert.doesNotMatch(dead,/alt="AK-47"/);
  assert.match(dead,/class="player-hp">—/);
  console.log('Team panel checks passed.');
} finally { await server.close(); }
