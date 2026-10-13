import { useTranslation } from 'react-i18next';
import { useCallback } from 'react';
import { Badge, Button, Flex } from '@radix-ui/themes';
import { useAppTheme } from '../../AppTheme';
import { useStudio } from '../../StudioProvider';
import type { DraftGuard, SettingsSection, StudioRoute } from '../../studioState';
import { AutomationView } from '../AutomationView';
import { SettingsView } from '../SettingsView';
import { AboutDialog } from '../AboutDialog';
import { Brand } from './Brand';
import { Icon } from './Icon';
import { APP_VERSION } from '../../appVersion';
import { CommunitySettings } from './CommunitySettings';

const SECTIONS: {id:SettingsSection;name:string;description:string}[]=[
  {id:'connection',name:'Connection',description:'Your Steam match history'},
  {id:'automation',name:'Automation',description:'Choose what runs automatically'},
  {id:'ai',name:'AI',description:'Local models and blooper review'},
  {id:'community',name:'Community & learning',description:'Feedback, contributions and model updates'},
  {id:'recording',name:'Recording',description:'Default export quality'},
  {id:'appearance',name:'Appearance',description:'Theme, text size and language'},
  {id:'tools',name:'Tools',description:'Game, recording tools and diagnostics'},
  {id:'storage',name:'Storage',description:'Demo sources, caches and files'},
  {id:'about',name:'About',description:'Source, licenses and build information'}
];
export function StudioSettings({section='connection',active=true,onNavigate,onDraft,toolsRequest=0,toolsTarget='render'}: {section?:SettingsSection;active?:boolean;onNavigate:(route:StudioRoute)=>void;onDraft:(key:string,guard:DraftGuard|undefined)=>void;toolsRequest?:number;toolsTarget?:'render'|'replay'}) {
  const {t}=useTranslation();
  const studio=useStudio();
  const theme=useAppTheme();
  const automationGuard=useCallback((guard:DraftGuard|undefined)=>onDraft('automation',guard),[onDraft]);
  const nativeGuard=useCallback((guard:DraftGuard|undefined)=>onDraft('native',guard),[onDraft]);
  const communityGuard=useCallback((guard:DraftGuard|undefined)=>onDraft('community',guard),[onDraft]);
  const automationSection=['connection','automation','ai'].includes(section);
  const nativeSection=['recording','appearance','tools','storage'].includes(section);
  // Completed/registered outputs change storage; queue progress does not.
  const storageKey=studio.jobs.flatMap(job=>job.outputs.map(output=>`${job.id}:${output.file}:${output.bytes}`)).sort().join('|');
  return <div className="studio-page settings-shell">
    <nav className="settings-nav" aria-label="Settings sections">{SECTIONS.map(item=><button key={item.id} aria-current={item.id===section?'page':undefined} onClick={()=>onNavigate({page:'settings',section:item.id})}><span>{t(`studio.section${item.id.charAt(0).toUpperCase()+item.id.slice(1)}`,{defaultValue:item.name})}</span><small>{t(`studio.section${item.id.charAt(0).toUpperCase()+item.id.slice(1)}Description`,{defaultValue:item.description})}</small></button>)}</nav>
    <div className="settings-content">
      <div hidden={!automationSection}><AutomationView section={automationSection?section as 'connection'|'automation'|'ai':'connection'} onGuardChange={automationGuard}/></div>
      <div hidden={section!=='community'}><CommunitySettings onGuardChange={communityGuard}/></div>
      {section==='appearance'&&<section className="studio-card"><h2>{t('studio.appearanceTitle', { defaultValue: "Your studio, your way" })}</h2><p>{t('studio.appearanceDescription', { defaultValue: "Choose a comfortable appearance. These preferences save immediately." })}</p><h3>{t('studio.colorTheme', { defaultValue: "Color theme" })}</h3><Flex gap="3" wrap="wrap"><Button variant={theme.appearance==='dark'?'solid':'outline'} onClick={()=>{if(theme.appearance!=='dark')theme.toggle();}}>{t('studio.darkStudio', { defaultValue: "Dark studio" })}</Button><Button variant={theme.appearance==='light'?'solid':'outline'} onClick={()=>{if(theme.appearance!=='light')theme.toggle();}}>{t('studio.lightStudio', { defaultValue: "Light studio" })}</Button></Flex><h3>{t('studio.textSize', { defaultValue: "Text size" })}</h3><Flex gap="3" wrap="wrap">{(['small','medium','large'] as const).map((size,index)=><Button key={size} variant={theme.fontSize===size?'solid':'outline'} onClick={()=>theme.setFontSize(size)}>{[t('studio.textStandard', { defaultValue: "Standard" }),t('studio.textComfortable', { defaultValue: "Comfortable" }),t('studio.textLarge', { defaultValue: "Large" })][index]}</Button>)}</Flex><p className="studio-caption">{t('studio.motionDescription', { defaultValue: "Motion follows your Windows reduced-motion preference." })}</p></section>}
      <div hidden={!nativeSection}><SettingsView active={active&&nativeSection} storageKey={storageKey} onChanged={studio.refresh} group={nativeSection?section as 'recording'|'appearance'|'tools'|'storage':'tools'} onGuardChange={nativeGuard} toolsRequest={toolsRequest} toolsTarget={toolsTarget}/></div>
      {section==='about'&&<section className="studio-card studio-about"><Brand/><h2>{t('studio.aboutTagline', { defaultValue: "Your gameplay, on your own terms." })}</h2><p>{t('studio.aboutDescription', { defaultValue: "ClipForge is a local Windows CS2 demo and clip studio, built on the open-source CS DemoDesk foundation by NOIH." })}</p><Badge>{t('studio.versionLabel',{defaultValue:'Version {{version}}',version:studio.status?.version||APP_VERSION})}</Badge><p>{t('studio.workspaceFilesDescription', { defaultValue: "Your models, demos, caches and recordings stay in your ClipForge folder." })}</p><Flex align="center" gap="3"><Icon name="info"/><AboutDialog triggerLabel={t('studio.sourceCreditsLicenses', { defaultValue: "Source, credits and licenses" })}/></Flex></section>}
    </div>
  </div>;
}
