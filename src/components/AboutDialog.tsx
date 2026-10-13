import { Tooltip } from '@radix-ui/themes';
import { useEffect, useState } from 'react';
import { Badge, Button, Dialog, Flex, IconButton, Text } from '@radix-ui/themes';
import { useTranslation } from 'react-i18next';
import { api } from '../api.ts';
import logo from '../../src-tauri/icons/128x128.png';

const THIRD_PARTY: Array<{ name: string; repo: string; license: string; url: string }> = [
  { name: 'Qwen3-VL models', repo: 'Qwen/Qwen3-VL', license: 'Apache-2.0', url: 'https://github.com/QwenLM/Qwen3-VL' },
  { name: 'Inter typography', repo: 'rsms/inter · 4.1', license: 'OFL-1.1', url: 'https://github.com/rsms/inter/releases/tag/v4.1' },
  { name: 'Bootstrap Icons', repo: 'twbs/icons · 1.13.1', license: 'MIT', url: 'https://github.com/twbs/icons' },
  { name: 'CS DemoDesk (foundation)', repo: 'noih/cs-demodesk', license: 'AGPL-3.0', url: 'https://github.com/noih/cs-demodesk' },
  { name: 'HLAE', repo: 'advancedfx/advancedfx', license: 'custom', url: 'https://github.com/advancedfx/advancedfx' },
  { name: 'FFmpeg', repo: 'BtbN/FFmpeg-Builds', license: 'GPL', url: 'https://github.com/BtbN/FFmpeg-Builds' },
  { name: 'Source 2 Viewer', repo: 'ValveResourceFormat/ValveResourceFormat', license: 'MIT', url: 'https://github.com/ValveResourceFormat/ValveResourceFormat' },
  { name: 'FidelityFX CAS', repo: 'GPUOpen-Effects/FidelityFX-CAS', license: 'MIT', url: 'https://github.com/GPUOpen-Effects/FidelityFX-CAS' },
  { name: 'demoparser', repo: 'LaihoE/demoparser', license: 'MIT', url: 'https://github.com/LaihoE/demoparser' },
];

function LinkIcon({ url, label }: { url: string; label: string }) {
  return (
    <IconButton size="1" variant="ghost" color="gray" aria-label={label} onClick={() => void api.openUrl(url)}>
      <i aria-hidden="true" className="bi bi-box-arrow-up-right app-icon" />
    </IconButton>
  );
}

/** Local build identity and source/license attribution for this fork. */
export function AboutDialog({triggerLabel}: {triggerLabel?:string} = {}) {
  const { t } = useTranslation();
  const [version, setVersion] = useState<string>();
  useEffect(() => {
    let active = true;
    void api.status().then((s) => { if (active) setVersion(s.version); }).catch(() => undefined);
    return () => { active = false; };
  }, []);
  return (
    <Dialog.Root>
      <Tooltip delayDuration={150} content={t('about.button')}><Dialog.Trigger>
        {triggerLabel?<Button variant="outline">{triggerLabel}</Button>:<IconButton variant="ghost" color="gray" aria-label={t('about.button')}>
          <i aria-hidden="true" className="bi bi-info-circle app-icon" />
        </IconButton>}
      </Dialog.Trigger></Tooltip>
      <Dialog.Content maxWidth="800px" style={{ padding: 32 }}>
        <Flex justify="between" align="center" gap="3">
          <Flex align="center" gap="3">
            <img src={logo} alt="" width="48" height="48" style={{ background: 'var(--app-logo-background, #121518)', borderRadius: 10, padding: 4 }} />
            <Dialog.Title mb="0">ClipForge</Dialog.Title>
          </Flex>
          <Text size="2" color="gray" className="mono">v{version ?? '…'} · Local build</Text>
        </Flex>
        <Dialog.Description size="2" color="gray" mt="4" mb="0" style={{ lineHeight: 1.7 }}>
          Local CS2 demo and clip studio. Based on CS DemoDesk by NOIH.
        </Dialog.Description>
        <Flex align="center" gap="2" mt="5" wrap="wrap">
          <Text size="2">Local fork · upstream copyright © 2026 NOIH</Text>
          <Text size="2" color="gray">· AGPL-3.0</Text>
        </Flex>
        <Flex align="center" gap="4" mt="3" wrap="wrap">
          <Button size="2" variant="soft" color="gray" aria-label="ClipForge source code"
            onClick={() => void api.openUrl('https://github.com/The1-Master/ClipForge')}>
            ClipForge source <i aria-hidden="true" className="bi bi-box-arrow-up-right app-icon" />
          </Button>
          <Button size="2" variant="soft" color="gray" aria-label="Upstream source"
            onClick={() => void api.openUrl('https://github.com/noih/cs-demodesk/tree/a85e85130d80d72a3415ee8ee01ee94626c43e5f')}>
            Upstream source <i aria-hidden="true" className="bi bi-box-arrow-up-right app-icon" />
          </Button>
          <Button size="2" variant="soft" color="gray" aria-label="Source license"
            onClick={() => void api.openUrl('https://github.com/noih/cs-demodesk/blob/a85e85130d80d72a3415ee8ee01ee94626c43e5f/LICENSE')}>
            AGPL license <i aria-hidden="true" className="bi bi-box-arrow-up-right app-icon" />
          </Button>
        </Flex>
        <Text as="div" size="2" weight="medium" mt="5" mb="2">{t('about.thirdParty')}</Text>
        <div className="about-list">
          {THIRD_PARTY.map((component) => (
            <div key={component.name} className="about-row">
              <Text size="2">{component.name}</Text>
              <Text size="1" color="gray" className="mono" truncate title={component.repo}>{component.repo}</Text>
              <Badge size="1" variant="soft" color="gray">{component.license}</Badge>
              <LinkIcon url={component.url} label={t('about.openSite', { name: component.name })} />
            </div>
          ))}
        </div>
        <Flex justify="end" mt="4">
          <Dialog.Close><Button variant="soft">{t('common.close')}</Button></Dialog.Close>
        </Flex>
      </Dialog.Content>
    </Dialog.Root>
  );
}
