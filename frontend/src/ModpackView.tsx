import { faArrowUp, faCodeBranch, faRotateRight, faSyncAlt, faXmark } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { Alert, Anchor, Badge, Button, Group, List, Loader, Modal, Paper, Progress, Stack, Text } from '@mantine/core';
import { useCallback, useEffect, useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import { useToast } from '@/providers/ToastProvider.tsx';
import * as api from './api';
import ProjectIcon from './ProjectIcon';
import { CurrentModpack, ModpackJob, ProviderId, Settings, Target } from './types';
import { formatDate, PROVIDER_LABELS } from './util';

interface Props {
  server: string;
  settings: Settings;
  target: Target;
  filters: React.ReactNode;
  /** Bumped by the page when an install was started, to start polling. */
  jobStarted: number;
  onOpen: (provider: ProviderId, id: string, tab: 'versions' | 'details') => void;
  onUpdate: (current: CurrentModpack) => void;
  /** Install the current version again, e.g. after mods were removed by hand. */
  onReinstall: (current: CurrentModpack) => void;
  /** The modpack was removed from the list. */
  onForgotten: () => void;
}

/** Manage → Modpacks: the pack this server was set up from, and the running install. */
export default function ModpackView({
  server,
  settings,
  target,
  filters,
  jobStarted,
  onOpen,
  onUpdate,
  onReinstall,
  onForgotten,
}: Props) {
  const [current, setCurrent] = useState<CurrentModpack | null | undefined>(undefined);
  const [job, setJob] = useState<ModpackJob | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmForget, setConfirmForget] = useState(false);
  const { addToast } = useToast();

  const loadCurrent = useCallback(() => {
    api
      .modpackCurrent(server, target)
      .then(setCurrent)
      .catch((e) => setError(httpErrorToHuman(e)));
  }, [server, target]);

  useEffect(loadCurrent, [loadCurrent]);

  // poll the job while it runs
  useEffect(() => {
    let alive = true;
    let timer: ReturnType<typeof setTimeout>;
    const tick = async () => {
      try {
        const j = await api.modpackJob(server);
        if (!alive) return;
        setJob(j);
        if (j?.state === 'running') {
          timer = setTimeout(tick, 1500);
        } else if (j) {
          loadCurrent();
        }
      } catch {
        if (alive) timer = setTimeout(tick, 5000);
      }
    };
    tick();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [server, jobStarted]);

  const running = job?.state === 'running';

  const forget = async () => {
    setConfirmForget(false);
    try {
      await api.modpackForget(server);
      setJob(null);
      setCurrent(null);
      onForgotten();
      addToast('Modpack removed from the list. No files were changed.', 'success');
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
    }
  };

  return (
    <Stack>
      <Group align='flex-end' gap='sm' wrap='wrap'>
        {filters}
        <Button
          variant='default'
          size='xs'
          leftSection={<FontAwesomeIcon icon={faSyncAlt} />}
          onClick={loadCurrent}
          ml='auto'
        >
          Check for updates
        </Button>
      </Group>

      {error && <Alert color='red'>{error}</Alert>}

      {job && (
        <Paper withBorder radius='md' p='md'>
          <Group justify='space-between' mb='xs'>
            <Text fw={500}>
              {job.title} {job.version}
            </Text>
            <Badge
              color={
                job.state === 'done'
                  ? job.failed.length
                    ? 'yellow'
                    : 'green'
                  : job.state === 'failed'
                    ? 'red'
                    : 'blue'
              }
            >
              {job.state === 'running'
                ? 'installing'
                : job.state === 'done'
                  ? job.failed.length
                    ? 'done with errors'
                    : 'installed'
                  : 'failed'}
            </Badge>
          </Group>
          {running && (
            <>
              <Progress value={job.total ? (job.done / job.total) * 100 : 5} animated mb={4} />
              <Text size='xs' c='dimmed'>
                {job.step} · {job.done} of {job.total || '?'}
              </Text>
            </>
          )}
          {job.state !== 'running' && (
            <Text size='sm' c={job.state === 'failed' ? 'red' : undefined}>
              {job.state === 'failed'
                ? job.step
                : `${job.installed} files downloaded, ${job.written} override files written${job.removed ? `, ${job.removed} old mods removed` : ''}${job.finished ? ` · ${new Date(job.finished * 1000).toLocaleString()}` : ''}. Restart the server to load it.`}
            </Text>
          )}
          {job.failed.length > 0 && (
            <Alert color='red' mt='sm' py='xs' title={`${job.failed.length} failed`}>
              <List size='xs'>
                {job.failed.slice(0, 50).map((f) => (
                  <List.Item key={f}>{f}</List.Item>
                ))}
              </List>
            </Alert>
          )}
          {job.manual.length > 0 && (
            <Alert color='yellow' mt='sm' py='xs' title='Download these by hand and upload them in the Files tab'>
              <List size='xs'>
                {job.manual.map((m) => (
                  <List.Item key={m.path}>
                    {m.title ?? m.path}{' '}
                    {m.page && (
                      <Anchor href={m.page} target='_blank' rel='noreferrer' size='xs'>
                        download
                      </Anchor>
                    )}{' '}
                    → /{m.path}
                  </List.Item>
                ))}
              </List>
            </Alert>
          )}
          {(job.switched ?? []).map((s) => (
            <Text key={s} size='xs' c='dimmed' mt={4}>
              {s}
            </Text>
          ))}
          {job.warnings.map((w) => (
            <Text key={w} size='xs' c='yellow' mt={4}>
              {w}
            </Text>
          ))}
          {job.skipped.length > 0 && (
            <Text size='xs' c='dimmed' mt='xs'>
              Left out {job.skipped.length} client-only: {job.skipped.join(', ')}
            </Text>
          )}
        </Paper>
      )}

      <Paper withBorder radius='md' p='md'>
        {current === undefined ? (
          <Group justify='center' py='md'>
            <Loader size='sm' />
          </Group>
        ) : current === null ? (
          <Text c='dimmed' ta='center' py='md'>
            No modpack was installed on this server with Mod Manager yet. Pick one under Browse Modpacks.
          </Text>
        ) : (
          <Group wrap='nowrap' align='flex-start'>
            <ProjectIcon url={current.project?.icon_url ?? null} title={current.title} size={56} />
            <Stack gap={2} style={{ flex: 1, minWidth: 0 }}>
              <Group gap='xs'>
                <Text fw={500}>{current.title}</Text>
                <Badge size='xs' variant='light'>
                  {PROVIDER_LABELS[current.provider]}
                </Badge>
              </Group>
              <Text size='sm'>
                {current.version_number}
                <Text span size='xs' c='dimmed'>
                  {' '}
                  · installed {formatDate(current.installed)}
                </Text>
              </Text>
              {current.update ? (
                <Text size='sm' c='green'>
                  Update available: {current.update.version_number}
                </Text>
              ) : (
                <Text size='xs' c='dimmed'>
                  Up to date for {target.game_version === 'any' ? 'this server' : `Minecraft ${target.game_version}`}
                </Text>
              )}
            </Stack>
            <Group gap='xs'>
              <Button
                variant='default'
                size='xs'
                leftSection={<FontAwesomeIcon icon={faCodeBranch} />}
                onClick={() => onOpen(current.provider, current.project_id, 'versions')}
              >
                Versions
              </Button>
              {settings.can_manage && (
                <Button
                  variant='default'
                  size='xs'
                  leftSection={<FontAwesomeIcon icon={faRotateRight} />}
                  disabled={running}
                  onClick={() => onReinstall(current)}
                >
                  Reinstall
                </Button>
              )}
              {settings.can_manage && (
                <Button
                  variant='default'
                  color='red'
                  size='xs'
                  leftSection={<FontAwesomeIcon icon={faXmark} />}
                  disabled={running}
                  onClick={() => setConfirmForget(true)}
                >
                  Remove from list
                </Button>
              )}
              {current.update && settings.can_manage && (
                <Button
                  size='xs'
                  color='green'
                  leftSection={<FontAwesomeIcon icon={faArrowUp} />}
                  disabled={running}
                  onClick={() => onUpdate(current)}
                >
                  Update
                </Button>
              )}
            </Group>
          </Group>
        )}
      </Paper>

      <Modal opened={confirmForget} onClose={() => setConfirmForget(false)} title='Remove modpack' centered>
        <Text size='sm'>
          Forget that this server runs <b>{current?.title}</b>? No files are changed. Use this after wiping or
          reinstalling the server, then install a modpack from Browse again.
        </Text>
        <Group justify='flex-end' mt='md'>
          <Button variant='default' onClick={() => setConfirmForget(false)}>
            Cancel
          </Button>
          <Button color='red' onClick={forget}>
            Remove from list
          </Button>
        </Group>
      </Modal>
    </Stack>
  );
}
