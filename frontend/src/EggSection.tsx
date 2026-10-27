import { faDownload, faEgg } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { Alert, Button, Divider, Group, Select, Stack, Text } from '@mantine/core';
import { useEffect, useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import { useToast } from '@/providers/ToastProvider.tsx';
import { addAnyLoaderEgg, downloadAnyLoaderEgg, EggStatus, getAnyLoaderEgg } from './api';

/** Admin settings: Mod Manager's "Minecraft: Java (any loader)" egg. */
export default function EggSection() {
  const { addToast } = useToast();
  const [status, setStatus] = useState<EggStatus | null>(null);
  const [nest, setNest] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = () =>
    getAnyLoaderEgg()
      .then((s) => {
        setStatus(s);
        // the nest that already holds Minecraft eggs
        const best = [...s.nests].sort((a, b) => b.minecraft_eggs - a.minecraft_eggs)[0];
        setNest((n) => n ?? best?.uuid ?? null);
      })
      .catch((e) => addToast(httpErrorToHuman(e), 'error'));

  useEffect(() => {
    load();
  }, []);

  const add = async () => {
    if (!nest) return;
    setBusy(true);
    try {
      await addAnyLoaderEgg(nest);
      addToast(status?.installed ? 'The egg is up to date.' : 'The egg was added.', 'success');
      await load();
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
    } finally {
      setBusy(false);
    }
  };

  const download = async () => {
    try {
      const blob = await downloadAnyLoaderEgg();
      const a = document.createElement('a');
      a.href = URL.createObjectURL(blob);
      a.download = 'egg-minecraft-any-loader.json';
      a.click();
      URL.revokeObjectURL(a.href);
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
    }
  };

  const installed = status?.installed;

  return (
    <Stack gap='xs'>
      <Divider label='Egg for any loader' labelPosition='left' />
      <Text size='sm' c='dimmed'>
        One Minecraft egg that installs vanilla, Fabric, Quilt, Forge, NeoForge, Paper or Purpur, chosen with a startup
        variable. Modpack installs on servers that use it switch the loader and Minecraft version by reinstalling,
        without moving the server to another egg; worlds, mods and configs are kept. Mod Manager also picks it when a
        server has to move to another egg.
      </Text>
      {status === null ? null : installed ? (
        <Group gap='sm'>
          <Text size='sm'>
            <FontAwesomeIcon icon={faEgg} /> <b>{installed.name}</b> is in the <b>{installed.nest_name}</b> nest
            {installed.outdated ? ` (revision ${installed.revision}, revision ${status.revision} is available)` : ''}.
          </Text>
          {installed.outdated && (
            <Button size='xs' loading={busy} onClick={add}>
              Update the egg
            </Button>
          )}
        </Group>
      ) : (
        <Group align='flex-end' gap='sm'>
          <Select
            label='Nest'
            size='xs'
            w={240}
            allowDeselect={false}
            value={nest}
            onChange={setNest}
            data={status.nests.map((n) => ({ value: n.uuid, label: n.name }))}
          />
          <Button
            size='xs'
            leftSection={<FontAwesomeIcon icon={faEgg} />}
            loading={busy}
            disabled={!nest}
            onClick={add}
          >
            Add the egg
          </Button>
        </Group>
      )}
      {installed && (
        <Alert color='gray' py='xs'>
          <Text size='xs'>
            To use it for an existing server, change the server's egg to it under Admin → Servers (or let a modpack
            install do it), set Loader and Minecraft Version under Startup, and reinstall.
          </Text>
        </Alert>
      )}
      <Group>
        <Button
          variant='subtle'
          size='compact-xs'
          leftSection={<FontAwesomeIcon icon={faDownload} />}
          onClick={download}
        >
          Download the egg file (to import on another panel)
        </Button>
      </Group>
    </Stack>
  );
}
