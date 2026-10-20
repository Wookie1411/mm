import {
  faArrowUp,
  faCodeBranch,
  faQuestionCircle,
  faSearch,
  faSyncAlt,
  faTrash,
} from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import {
  ActionIcon,
  Alert,
  Badge,
  Button,
  Group,
  Loader,
  Modal,
  Paper,
  SegmentedControl,
  Stack,
  Switch,
  Table,
  Text,
  TextInput,
  Tooltip,
} from '@mantine/core';
import { useMemo, useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import { useToast } from '@/providers/ToastProvider.tsx';
import * as api from './api';
import ModListTools from './ModListTools';
import ProjectIcon from './ProjectIcon';
import { Installed, InstalledList, InstallResult, ProviderId, Settings, Target } from './types';
import { applyHint, formatBytes, formatDate, PROVIDER_LABELS, versionTypeColor } from './util';

interface Props {
  server: string;
  settings: Settings;
  target: Target;
  filters: React.ReactNode;
  list: InstalledList | null;
  loading: boolean;
  onRefresh: (fresh: boolean) => void;
  onOpen: (provider: ProviderId, id: string, tab: 'versions' | 'details') => void;
  onInstalled: (result: InstallResult) => void;
}

type Filter = 'all' | 'updates' | 'disabled' | 'unknown';

export default function InstalledView({
  server,
  settings,
  target,
  filters,
  list,
  loading,
  onRefresh,
  onOpen,
  onInstalled,
}: Props) {
  const { addToast } = useToast();
  const [search, setSearch] = useState('');
  const [filter, setFilter] = useState<Filter>('all');
  const [busy, setBusy] = useState<string | null>(null);
  const [updatingAll, setUpdatingAll] = useState<{ done: number; total: number } | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<Installed | null>(null);

  const mods = list?.mods ?? [];
  const updates = mods.filter((m) => m.update && m.provider && m.project);
  const shown = useMemo(() => {
    const q = search.trim().toLowerCase();
    return mods.filter((m) => {
      if (filter === 'updates' && !m.update) return false;
      if (filter === 'disabled' && m.enabled) return false;
      if (filter === 'unknown' && m.project) return false;
      if (!q) return true;
      return m.file.toLowerCase().includes(q) || (m.project?.title ?? '').toLowerCase().includes(q);
    });
  }, [mods, search, filter]);

  const update = async (m: Installed) => {
    if (!m.update || !m.provider || !m.project) return;
    setBusy(m.file);
    try {
      onInstalled(
        await api.install(server, target, {
          provider: m.provider,
          project_id: m.project.id,
          version_id: m.update.id,
          replace: m.file,
          dependencies: true,
        }),
      );
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
    } finally {
      setBusy(null);
    }
  };

  const updateAll = async () => {
    setUpdatingAll({ done: 0, total: updates.length });
    const merged: InstallResult = { installed: [], warnings: [] };
    for (const [i, m] of updates.entries()) {
      try {
        const r = await api.install(server, target, {
          provider: m.provider!,
          project_id: m.project!.id,
          version_id: m.update!.id,
          replace: m.file,
          dependencies: true,
        });
        merged.installed.push(...r.installed);
        merged.warnings.push(...r.warnings);
      } catch (e) {
        merged.warnings.push(`${m.project!.title}: ${httpErrorToHuman(e)}`);
      }
      setUpdatingAll({ done: i + 1, total: updates.length });
    }
    setUpdatingAll(null);
    onInstalled(merged);
  };

  const toggle = async (m: Installed) => {
    setBusy(m.file);
    try {
      await api.toggle(server, target, m.file, !m.enabled);
      addToast(`${m.project?.title ?? m.file} ${m.enabled ? 'disabled' : 'enabled'}. ${applyHint(target)}`, 'success');
      onRefresh(false);
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
    } finally {
      setBusy(null);
    }
  };

  const remove = async (m: Installed) => {
    setConfirmRemove(null);
    setBusy(m.file);
    try {
      await api.remove(server, target, [m.file]);
      addToast(`${m.project?.title ?? m.file} removed. ${applyHint(target)}`, 'success');
      onRefresh(false);
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
    } finally {
      setBusy(null);
    }
  };

  const canManage = settings.can_manage;
  const locked = busy !== null || updatingAll !== null;

  return (
    <Stack>
      <Group align='flex-end' gap='sm' wrap='wrap'>
        {filters}
        <TextInput
          label='Search'
          style={{ flex: 1, minWidth: 200 }}
          placeholder='Filter installed…'
          leftSection={<FontAwesomeIcon icon={faSearch} />}
          value={search}
          onChange={(e) => setSearch(e.currentTarget.value)}
        />
      </Group>

      <Group justify='space-between'>
        <SegmentedControl
          size='xs'
          value={filter}
          onChange={(v) => setFilter(v as Filter)}
          data={[
            { value: 'all', label: `All (${mods.length})` },
            { value: 'updates', label: `Updates (${updates.length})` },
            { value: 'disabled', label: `Disabled (${mods.filter((m) => !m.enabled).length})` },
            { value: 'unknown', label: `Unrecognised (${mods.filter((m) => !m.project).length})` },
          ]}
        />
        <Group gap='xs'>
          {list && (
            <Text size='xs' c='dimmed'>
              /{list.directory} · checked {new Date(list.generated_at * 1000).toLocaleTimeString()}
            </Text>
          )}
          <ModListTools server={server} settings={settings} target={target} onInstalled={onInstalled} />
          <Button
            variant='default'
            size='xs'
            leftSection={<FontAwesomeIcon icon={faSyncAlt} spin={loading} />}
            disabled={loading || locked}
            onClick={() => onRefresh(true)}
          >
            Check for updates
          </Button>
          {canManage && updates.length > 0 && (
            <Button
              size='xs'
              color='green'
              leftSection={<FontAwesomeIcon icon={faArrowUp} />}
              loading={updatingAll !== null}
              disabled={locked && updatingAll === null}
              onClick={updateAll}
            >
              Update all ({updates.length})
            </Button>
          )}
        </Group>
      </Group>

      {updatingAll && (
        <Text size='sm' c='dimmed'>
          Updating {updatingAll.done + 1} of {updatingAll.total}…
        </Text>
      )}
      {list?.warnings.map((w) => (
        <Alert key={w} color='yellow' py='xs'>
          {w}
        </Alert>
      ))}

      <Paper withBorder radius='md'>
        {loading && !list ? (
          <Group justify='center' py='xl'>
            <Loader />
          </Group>
        ) : !list?.directory ? (
          <Text c='dimmed' ta='center' py='xl'>
            Could not tell whether this server uses mods or plugins. Pick a loader above.
          </Text>
        ) : shown.length === 0 ? (
          <Text c='dimmed' ta='center' py='xl'>
            {mods.length === 0
              ? `Nothing in /${list.directory} yet. Install something from the Browse tab.`
              : 'No matches.'}
          </Text>
        ) : (
          <Table.ScrollContainer minWidth={760}>
            <Table verticalSpacing='sm' highlightOnHover>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Name</Table.Th>
                  <Table.Th>Version</Table.Th>
                  <Table.Th>Update</Table.Th>
                  <Table.Th>Enabled</Table.Th>
                  <Table.Th />
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {shown.map((m) => (
                  <Table.Tr key={m.file} style={{ opacity: m.enabled ? 1 : 0.6 }}>
                    <Table.Td>
                      <Group gap='sm' wrap='nowrap'>
                        <ProjectIcon url={m.project?.icon_url ?? null} title={m.project?.title ?? m.file} size={36} />
                        <div style={{ minWidth: 0 }}>
                          <Group gap={6} wrap='nowrap'>
                            <Text size='sm' fw={500} truncate maw={280}>
                              {m.project?.title ?? m.file}
                            </Text>
                            {m.provider ? (
                              <Badge size='xs' variant='light'>
                                {PROVIDER_LABELS[m.provider]}
                              </Badge>
                            ) : (
                              <Tooltip label='Not found on Modrinth or CurseForge (uploaded by hand, or a private build)'>
                                <Badge
                                  size='xs'
                                  variant='default'
                                  leftSection={<FontAwesomeIcon icon={faQuestionCircle} />}
                                >
                                  unrecognised
                                </Badge>
                              </Tooltip>
                            )}
                          </Group>
                          <Text size='xs' c='dimmed' truncate maw={360} title={m.file}>
                            {m.file} · {formatBytes(m.size)}
                          </Text>
                        </div>
                      </Group>
                    </Table.Td>
                    <Table.Td>
                      {m.version ? (
                        <Group gap={6} wrap='nowrap'>
                          <Text size='sm' truncate maw={180} title={m.version.name}>
                            {m.version.version_number}
                          </Text>
                          {m.version.version_type !== 'release' && (
                            <Badge size='xs' variant='light' color={versionTypeColor(m.version.version_type)}>
                              {m.version.version_type}
                            </Badge>
                          )}
                        </Group>
                      ) : (
                        <Text size='sm' c='dimmed'>
                          —
                        </Text>
                      )}
                      {m.version?.published && (
                        <Text size='xs' c='dimmed'>
                          {formatDate(m.version.published)}
                        </Text>
                      )}
                    </Table.Td>
                    <Table.Td>
                      {m.update ? (
                        <Group gap='xs' wrap='nowrap'>
                          <Tooltip label={`${m.update.name} · ${formatDate(m.update.published)}`}>
                            <Text size='sm' c='green' truncate maw={160}>
                              {m.update.version_number}
                            </Text>
                          </Tooltip>
                          {canManage && (
                            <Button
                              size='compact-xs'
                              color='green'
                              leftSection={<FontAwesomeIcon icon={faArrowUp} />}
                              loading={busy === m.file}
                              disabled={locked && busy !== m.file}
                              onClick={() => update(m)}
                            >
                              Update
                            </Button>
                          )}
                        </Group>
                      ) : (
                        <Text size='xs' c='dimmed'>
                          {m.project ? 'Up to date' : '—'}
                        </Text>
                      )}
                    </Table.Td>
                    <Table.Td>
                      <Switch
                        size='sm'
                        checked={m.enabled}
                        disabled={!canManage || locked}
                        onChange={() => toggle(m)}
                        aria-label={m.enabled ? 'Disable' : 'Enable'}
                      />
                    </Table.Td>
                    <Table.Td>
                      <Group gap={6} justify='flex-end' wrap='nowrap'>
                        {m.provider && m.project && (
                          <Tooltip label='Versions: update, downgrade or reinstall'>
                            <ActionIcon
                              variant='default'
                              onClick={() => onOpen(m.provider!, m.project!.id, 'versions')}
                            >
                              <FontAwesomeIcon icon={faCodeBranch} />
                            </ActionIcon>
                          </Tooltip>
                        )}
                        {canManage && (
                          <Tooltip label='Remove'>
                            <ActionIcon
                              variant='default'
                              color='red'
                              disabled={locked}
                              onClick={() => setConfirmRemove(m)}
                            >
                              <FontAwesomeIcon icon={faTrash} />
                            </ActionIcon>
                          </Tooltip>
                        )}
                      </Group>
                    </Table.Td>
                  </Table.Tr>
                ))}
              </Table.Tbody>
            </Table>
          </Table.ScrollContainer>
        )}
      </Paper>

      <Modal opened={confirmRemove !== null} onClose={() => setConfirmRemove(null)} title='Remove' centered>
        <Text size='sm'>
          Delete <b>{confirmRemove?.project?.title ?? confirmRemove?.file}</b> ({confirmRemove?.file}) from /
          {list?.directory}? Its config files are kept.
        </Text>
        <Group justify='flex-end' mt='md'>
          <Button variant='default' onClick={() => setConfirmRemove(null)}>
            Cancel
          </Button>
          <Button color='red' onClick={() => confirmRemove && remove(confirmRemove)}>
            Remove
          </Button>
        </Group>
      </Modal>
    </Stack>
  );
}
