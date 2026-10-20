import { faBoxOpen, faTriangleExclamation } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import {
  Alert,
  Anchor,
  Badge,
  Button,
  Checkbox,
  Group,
  List,
  Loader,
  Modal,
  ScrollArea,
  Stack,
  Text,
} from '@mantine/core';
import { useEffect, useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import { useToast } from '@/providers/ToastProvider.tsx';
import * as api from './api';
import { ModpackPlan, ProviderId } from './types';
import { formatBytes, loaderLabel } from './util';

interface Props {
  server: string;
  provider: ProviderId;
  projectId: string;
  versionId: string;
  title: string;
  /** Replacing the pack this server already runs: clean the mods folder by default. */
  replacing: boolean;
  onClose: () => void;
  onStarted: () => void;
}

const Section = ({ title, items }: { title: string; items: React.ReactNode[] }) =>
  items.length === 0 ? null : (
    <div>
      <Text size='sm' fw={500}>
        {title} ({items.length})
      </Text>
      <ScrollArea.Autosize mah={110}>
        <List size='xs' c='dimmed'>
          {items.map((item, i) => (
            <List.Item key={i}>{item}</List.Item>
          ))}
        </List>
      </ScrollArea.Autosize>
    </div>
  );

function switchDescription(plan: ModpackPlan): string | null {
  const how = plan.switch_version.how;
  if (!how) return plan.switch_version.reason;
  if (how.route === 'jar') {
    return 'Replaces the server jar, sets the version startup variables and picks the Java image this version needs.';
  }
  return `${
    how.moves ? `Moves this server to the ${how.egg} egg` : `Sets the ${how.egg} egg's version variables`
  }, picks the Java image this version needs and reinstalls the server with it. Your files are kept; the server is offline until it finishes.`;
}

/** Shows what installing a modpack version would do, then starts the install job. */
export default function ModpackDialog({
  server,
  provider,
  projectId,
  versionId,
  title,
  replacing,
  onClose,
  onStarted,
}: Props) {
  const { addToast } = useToast();
  const [plan, setPlan] = useState<ModpackPlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [clean, setClean] = useState(replacing);
  const [switchVersion, setSwitchVersion] = useState(false);
  const [wipe, setWipe] = useState(false);
  const [useServerPack, setUseServerPack] = useState(true);
  const [starting, setStarting] = useState(false);

  useEffect(() => {
    api
      .modpackPlan(server, { provider, project_id: projectId, version_id: versionId })
      .then((p) => {
        setPlan(p);
        if (p.warnings.length > 0 && p.switch_version.supported) setSwitchVersion(true);
        if (p.existing_mods > 0 && replacing) setClean(true);
      })
      .catch((e) => setError(httpErrorToHuman(e)));
  }, [server, provider, projectId, versionId]);

  const start = async () => {
    setStarting(true);
    try {
      await api.modpackInstall(server, {
        provider,
        project_id: projectId,
        version_id: versionId,
        clean,
        switch_version: switchVersion,
        wipe,
        server_pack: useServerPack,
      });
      onStarted();
      onClose();
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
    } finally {
      setStarting(false);
    }
  };

  return (
    <Modal opened onClose={onClose} size='lg' centered title={`Install modpack: ${title}`}>
      {error ? (
        <Alert color='red'>{error}</Alert>
      ) : !plan ? (
        <Group justify='center' py='xl' gap='sm'>
          <Loader size='sm' />
          <Text size='sm' c='dimmed'>
            The server's node is downloading and unpacking the modpack…
          </Text>
        </Group>
      ) : (
        <Stack gap='sm'>
          <Group gap='xs'>
            <Badge variant='light'>{plan.version.version_number}</Badge>
            <Text size='sm'>
              needs <b>{plan.requires.minecraft ? `Minecraft ${plan.requires.minecraft}` : 'any Minecraft'}</b>
              {plan.requires.loader && (
                <>
                  {' '}
                  with{' '}
                  <b>
                    {loaderLabel(plan.requires.loader)} {plan.requires.loader_version ?? ''}
                  </b>
                </>
              )}
              ; this server runs {loaderLabel(plan.server.loader)} {plan.server.game_version ?? ''}.
            </Text>
          </Group>

          {plan.warnings.map((w) => (
            <Alert key={w} color='yellow' py='xs' icon={<FontAwesomeIcon icon={faTriangleExclamation} />}>
              {w}{' '}
              {switchVersion
                ? 'It will be switched by the option below.'
                : "Change the server's egg / startup variables to match before starting it."}
            </Alert>
          ))}

          {plan.server_pack && (
            <Checkbox
              label={`Use the server pack: ${plan.server_pack.name} (${formatBytes(plan.server_pack.size)})`}
              description={
                plan.server_pack.url
                  ? 'Recommended. The pack author made it for servers, with client-only mods already left out. Wings downloads and unpacks it on the node.'
                  : 'The author does not allow third-party downloads of the server pack, so the regular pack is installed.'
              }
              disabled={!plan.server_pack.url}
              checked={useServerPack && !!plan.server_pack.url}
              onChange={(e) => setUseServerPack(e.currentTarget.checked)}
            />
          )}

          <Text size='sm' c={plan.server_pack?.url && useServerPack ? 'dimmed' : undefined}>
            {plan.server_pack?.url && useServerPack ? 'Instead of the regular pack: ' : ''}
            <FontAwesomeIcon icon={faBoxOpen} /> {plan.downloads.length} files to download (
            {formatBytes(plan.download_bytes)}) and {plan.overrides} config / override files to write (
            {formatBytes(plan.override_bytes)}). Existing files with the same name are overwritten.
          </Text>

          <Section title='Left out: client-only' items={plan.skipped.map((s) => s)} />
          <Section
            title='Download by hand (the author blocks third-party downloads)'
            items={plan.manual.map((m) => (
              <>
                {m.title ?? m.path}{' '}
                {m.page && (
                  <Anchor href={m.page} target='_blank' rel='noreferrer' size='xs'>
                    download
                  </Anchor>
                )}{' '}
                → /{m.path}
              </>
            ))}
          />
          <Section title='Left out: unsafe path or download host' items={plan.rejected} />

          <Checkbox
            color='red'
            label='Delete all server files first'
            description='Reinstalls the server and deletes everything on it (worlds, configs, mods) before the pack is installed. Make a backup first.'
            checked={wipe}
            onChange={(e) => setWipe(e.currentTarget.checked)}
          />

          {plan.existing_mods > 0 && !wipe && (
            <Checkbox
              label={`Remove the ${plan.existing_mods} mods currently in /mods first`}
              description='Recommended when switching or updating modpacks, so old mods do not clash with the new ones.'
              checked={clean}
              onChange={(e) => setClean(e.currentTarget.checked)}
            />
          )}

          {plan.requires.minecraft && (
            <Checkbox
              label={`Also install Minecraft ${plan.requires.minecraft}${
                plan.requires.loader
                  ? ` with ${loaderLabel(plan.requires.loader)} ${plan.requires.loader_version ?? ''}`.trimEnd()
                  : ''
              }`}
              description={switchDescription(plan)}
              disabled={!plan.switch_version.supported}
              checked={switchVersion && plan.switch_version.supported}
              onChange={(e) => setSwitchVersion(e.currentTarget.checked)}
            />
          )}

          <Text size='xs' c='dimmed'>
            Stop the server before installing. The install runs in the background; you can follow it in the Manage tab.
          </Text>

          <Group justify='flex-end'>
            <Button variant='default' onClick={onClose}>
              Cancel
            </Button>
            <Button
              onClick={start}
              loading={starting}
              color={wipe ? 'red' : undefined}
              disabled={plan.downloads.length + plan.overrides === 0}
            >
              {wipe ? 'Delete files and install' : 'Install modpack'}
            </Button>
          </Group>
        </Stack>
      )}
    </Modal>
  );
}
