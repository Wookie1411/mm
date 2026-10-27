import { Alert, Anchor, Button, Group, MultiSelect, PasswordInput, Stack, Switch, Text } from '@mantine/core';
import { useEffect, useMemo, useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import { useToast } from '@/providers/ToastProvider.tsx';
import AccessSetting from './AccessSetting';
import {
  AdminEgg,
  clearSettingsCache,
  getAdminEggs,
  getAdminSettings,
  KeyTest,
  AdminSettings as Settings,
  testCurseforgeKey,
  updateAdminSettings,
} from './api';
import EggSection from './EggSection';
import UpdateSection from './UpdateSection';

// Java servers (Vanilla too, for data packs); not Bedrock
const JAVA =
  /vanilla minecraft|java edition|paper|spigot|bukkit|purpur|folia|fabric|forge|quilt|sponge|velocity|bungee|waterfall|modpack|mohist|arclight/i;

/** Admin → Extensions → Mod Manager → Configure. */
export default function AdminSettings() {
  const { addToast } = useToast();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [eggs, setEggs] = useState<AdminEgg[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  /** null = leave the stored key alone. */
  const [key, setKey] = useState<string | null>(null);
  const [testing, setTesting] = useState(false);
  const [test, setTest] = useState<KeyTest | null>(null);

  const runTest = async () => {
    setTesting(true);
    setTest(null);
    try {
      setTest(await testCurseforgeKey(key));
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
    } finally {
      setTesting(false);
    }
  };

  useEffect(() => {
    Promise.all([getAdminSettings(), getAdminEggs()])
      .then(([s, e]) => {
        setSettings(s);
        setEggs(e);
      })
      .catch((e) => setError(httpErrorToHuman(e)));
  }, []);

  const eggData = useMemo(() => {
    const groups = new Map<string, { value: string; label: string }[]>();
    for (const egg of eggs) {
      const list = groups.get(egg.nest) ?? [];
      list.push({ value: egg.uuid, label: egg.name });
      groups.set(egg.nest, list);
    }
    return [...groups.entries()].map(([group, items]) => ({ group, items }));
  }, [eggs]);

  if (error) {
    return <Alert color='red'>{error}</Alert>;
  }
  if (!settings) {
    return <Text c='dimmed'>Loading…</Text>;
  }

  const save = async () => {
    setSaving(true);
    try {
      const { curseforge_key_set: _, ...rest } = settings;
      setSettings(await updateAdminSettings(key === null ? rest : { ...rest, curseforge_api_key: key }));
      setKey(null);
      clearSettingsCache();
      addToast('Mod Manager settings saved.', 'success');
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
    } finally {
      setSaving(false);
    }
  };

  return (
    <Stack maw={720}>
      <Text size='sm' c='dimmed'>
        Browse Modrinth and CurseForge from a server's Mods page and install, update, downgrade, disable or remove mods
        and plugins. Wings downloads the files straight from the provider. Viewing needs Read file contents or the Mod
        Manager permission; changes need Mod Manager &rarr; manage, or both Create and Delete files.
      </Text>
      <AccessSetting
        access={settings.access}
        users={settings.users}
        onChange={(access, users) => setSettings({ ...settings, access, users })}
      />
      <MultiSelect
        label='Eggs'
        description='The page works on servers with these eggs. None selected = every egg.'
        data={eggData}
        value={settings.eggs}
        onChange={(value) => setSettings({ ...settings, eggs: value })}
        searchable
        clearable
      />
      <Group gap='xs'>
        <Button
          variant='default'
          size='xs'
          onClick={() =>
            setSettings({
              ...settings,
              eggs: eggs.filter((e) => JAVA.test(`${e.name} ${e.nest}`)).map((e) => e.uuid),
            })
          }
        >
          Select all Minecraft Java eggs
        </Button>
        <Button variant='default' size='xs' onClick={() => setSettings({ ...settings, eggs: [] })}>
          All eggs
        </Button>
      </Group>
      <Switch
        label='Let modpack installs change the server type'
        description='When a pack needs another loader (e.g. NeoForge on a Fabric server), "Also install Minecraft" moves the server to a matching egg from the list above and reinstalls it, keeping its files.'
        checked={settings.switch_eggs}
        onChange={(e) => setSettings({ ...settings, switch_eggs: e.currentTarget.checked })}
      />
      <Switch
        label='Modrinth'
        description='No key needed.'
        checked={settings.modrinth_enabled}
        onChange={(e) => setSettings({ ...settings, modrinth_enabled: e.currentTarget.checked })}
      />
      <Switch
        label='CurseForge'
        description='Needs an API key.'
        checked={settings.curseforge_enabled}
        onChange={(e) => setSettings({ ...settings, curseforge_enabled: e.currentTarget.checked })}
      />
      <PasswordInput
        label='CurseForge API key'
        description={
          <>
            Create one at{' '}
            <Anchor href='https://console.curseforge.com/' target='_blank' rel='noreferrer' size='xs'>
              console.curseforge.com
            </Anchor>
            . Stored encrypted and never sent to browsers.{' '}
            {settings.curseforge_key_set ? 'A key is set; type a new one to replace it.' : 'No key is set.'}
          </>
        }
        placeholder={settings.curseforge_key_set ? '•••••••• (unchanged)' : '$2a$10$…'}
        value={key ?? ''}
        onChange={(e) => {
          setKey(e.currentTarget.value);
          setTest(null);
        }}
        disabled={!settings.curseforge_enabled}
      />
      <Group gap='xs'>
        <Button
          variant='default'
          size='xs'
          loading={testing}
          disabled={!settings.curseforge_enabled || (!key && !settings.curseforge_key_set)}
          onClick={runTest}
        >
          {key ? 'Test this key' : 'Test the stored key'}
        </Button>
        <Text size='xs' c='dimmed'>
          Asks CurseForge whether it accepts the key; the key is never shown.
        </Text>
      </Group>
      {test && (
        <Alert color={test.ok ? 'green' : 'red'} py='xs'>
          <Text size='sm'>
            {test.source === 'typed' ? 'Typed key' : 'Stored key'}: {test.message}
          </Text>
          {!test.ok && test.status !== null && (
            <Text size='xs' c='dimmed' mt={4}>
              Key length {test.shape.length}
              {test.shape.looks_like_a_key
                ? ', format looks right'
                : ' — CurseForge keys are usually 60 characters starting with $2a$10$; check the copy'}
              {test.shape.has_whitespace ? '; it contains spaces or line breaks' : ''}.
            </Text>
          )}
        </Alert>
      )}
      <Group justify='space-between'>
        {settings.curseforge_key_set ? (
          <Button variant='subtle' color='red' size='xs' onClick={() => setKey('')}>
            Remove the stored key on save
          </Button>
        ) : (
          <span />
        )}
        <Button onClick={save} loading={saving}>
          Save
        </Button>
      </Group>
      <EggSection />
      <UpdateSection />
    </Stack>
  );
}
