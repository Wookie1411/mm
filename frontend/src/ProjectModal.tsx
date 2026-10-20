import {
  faArrowDown,
  faArrowUp,
  faCheck,
  faDownload,
  faExternalLinkAlt,
  faHeart,
  faRightLeft,
  faRotateRight,
} from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import {
  Alert,
  Anchor,
  Badge,
  Button,
  Checkbox,
  Group,
  Loader,
  Modal,
  ScrollArea,
  Stack,
  Table,
  Tabs,
  Text,
  Tooltip,
  Typography,
} from '@mantine/core';
import { useEffect, useMemo, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import { httpErrorToHuman } from '@/api/axios.ts';
import { useToast } from '@/providers/ToastProvider.tsx';
import * as api from './api';
import ProjectIcon from './ProjectIcon';
import { Installed, InstallResult, ProjectDetail, ProviderId, Target, Version } from './types';
import {
  formatBytes,
  formatDate,
  formatNumber,
  loaderLabel,
  PROVIDER_LABELS,
  shortVersions,
  versionTypeColor,
} from './util';

interface Props {
  server: string;
  target: Target;
  provider: ProviderId;
  projectId: string;
  /** The jar of this project that is in the folder, if any. */
  installed: Installed | null;
  canManage: boolean;
  initialTab?: 'versions' | 'details';
  onClose: () => void;
  onInstalled: (result: InstallResult) => void;
  /** Modpacks: a version was picked; the page shows the modpack install dialog. */
  onModpack?: (version: Version, title: string) => void;
}

/** `reinstallable`: modpacks, whose installed version can be set up again. */
const actionFor = (v: Version, installed: Installed | null, reinstallable: boolean) => {
  if (!installed?.version) return { label: 'Install', icon: faDownload, color: 'blue' };
  if (installed.version.id === v.id) {
    return reinstallable ? { label: 'Reinstall', icon: faRotateRight, color: 'blue' } : null;
  }
  const current = installed.version.published ? Date.parse(installed.version.published) : 0;
  const candidate = v.published ? Date.parse(v.published) : 0;
  if (candidate > current) return { label: 'Update', icon: faArrowUp, color: 'green' };
  if (candidate < current) return { label: 'Downgrade', icon: faArrowDown, color: 'orange' };
  return { label: 'Switch', icon: faRightLeft, color: 'blue' };
};

export default function ProjectModal({
  server,
  target,
  provider,
  projectId,
  installed,
  canManage,
  initialTab = 'versions',
  onClose,
  onInstalled,
  onModpack,
}: Props) {
  const { addToast } = useToast();
  const [tab, setTab] = useState<string | null>(initialTab);
  const [project, setProject] = useState<ProjectDetail | null>(null);
  const [versions, setVersions] = useState<Version[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [allGameVersions, setAllGameVersions] = useState(false);
  const [withDependencies, setWithDependencies] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    api
      .getProject(server, provider, projectId)
      .then(setProject)
      .catch((e) => setError(httpErrorToHuman(e)));
  }, [server, provider, projectId]);

  useEffect(() => {
    setVersions(null);
    api
      .getVersions(server, provider, projectId, {
        ...target,
        game_version: allGameVersions ? 'any' : target.game_version,
      })
      .then(setVersions)
      .catch((e) => setError(httpErrorToHuman(e)));
  }, [server, provider, projectId, target.kind, target.loader, target.game_version, allGameVersions]);

  const run = async (v: Version) => {
    if (onModpack) {
      onModpack(v, project?.title ?? installed?.project?.title ?? 'Modpack');
      onClose();
      return;
    }
    setBusy(v.id);
    try {
      const result = await api.install(server, target, {
        provider,
        project_id: projectId,
        version_id: v.id,
        replace: installed?.file ?? null,
        dependencies: withDependencies,
      });
      onInstalled(result);
      onClose();
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
    } finally {
      setBusy(null);
    }
  };

  const title = project?.title ?? installed?.project?.title ?? 'Project';
  const requiredDeps = useMemo(
    () => (versions ?? []).some((v) => v.dependencies.some((d) => d.kind === 'required')),
    [versions],
  );

  return (
    <Modal opened onClose={onClose} size='xl' centered title={null} withCloseButton={false} padding='lg'>
      <Group align='flex-start' wrap='nowrap' mb='md'>
        <ProjectIcon url={project?.icon_url ?? installed?.project?.icon_url ?? null} title={title} size={64} />
        <Stack gap={4} style={{ flex: 1, minWidth: 0 }}>
          <Group gap='xs'>
            <Text fw={600} size='lg'>
              {title}
            </Text>
            <Badge variant='light' size='sm'>
              {PROVIDER_LABELS[provider]}
            </Badge>
          </Group>
          {project && (
            <Group gap='md'>
              <Text size='sm' c='dimmed'>
                <FontAwesomeIcon icon={faDownload} /> {formatNumber(project.downloads)}
              </Text>
              <Text size='sm' c='dimmed'>
                <FontAwesomeIcon icon={faHeart} /> {formatNumber(project.follows)}
              </Text>
              {project.author && (
                <Text size='sm' c='dimmed'>
                  by {project.author}
                </Text>
              )}
              <Anchor href={project.url} target='_blank' rel='noreferrer' size='sm'>
                <FontAwesomeIcon icon={faExternalLinkAlt} /> {PROVIDER_LABELS[provider]}
              </Anchor>
              {project.links.map((l) => (
                <Anchor key={l.url} href={l.url} target='_blank' rel='noreferrer' size='sm'>
                  {l.label}
                </Anchor>
              ))}
            </Group>
          )}
          <Text size='sm'>{project?.description}</Text>
        </Stack>
      </Group>

      {error && (
        <Alert color='red' mb='md'>
          {error}
        </Alert>
      )}
      {project?.distribution_blocked && (
        <Alert color='yellow' mb='md'>
          The author does not allow downloads through third-party apps. Download it from{' '}
          <Anchor href={project.url} target='_blank' rel='noreferrer'>
            CurseForge
          </Anchor>{' '}
          and upload it in the Files tab.
        </Alert>
      )}

      <Tabs value={tab} onChange={setTab}>
        <Tabs.List mb='sm'>
          <Tabs.Tab value='versions'>Versions</Tabs.Tab>
          <Tabs.Tab value='details'>Description</Tabs.Tab>
        </Tabs.List>

        <Tabs.Panel value='versions'>
          <Group justify='space-between' mb='xs'>
            <Text size='xs' c='dimmed'>
              {loaderLabel(target.loader === 'any' ? null : target.loader)}
              {target.game_version !== 'any' && !allGameVersions ? ` · Minecraft ${target.game_version}` : ''}
              {installed?.version && ` · installed: ${installed.version.version_number}`}
            </Text>
            <Group gap='md'>
              {canManage && requiredDeps && !onModpack && (
                <Checkbox
                  size='xs'
                  label='Install required dependencies'
                  checked={withDependencies}
                  onChange={(e) => setWithDependencies(e.currentTarget.checked)}
                />
              )}
              {target.game_version !== 'any' && (
                <Checkbox
                  size='xs'
                  label='All Minecraft versions'
                  checked={allGameVersions}
                  onChange={(e) => setAllGameVersions(e.currentTarget.checked)}
                />
              )}
            </Group>
          </Group>
          {versions === null ? (
            <Group justify='center' py='xl'>
              <Loader size='sm' />
            </Group>
          ) : versions.length === 0 ? (
            <Text size='sm' c='dimmed' py='md' ta='center'>
              No versions for this loader{allGameVersions ? '' : ' and Minecraft version'}.
            </Text>
          ) : (
            <ScrollArea.Autosize mah={420}>
              <Table striped highlightOnHover verticalSpacing='xs'>
                <Table.Thead>
                  <Table.Tr>
                    <Table.Th>Version</Table.Th>
                    <Table.Th>Minecraft</Table.Th>
                    <Table.Th>Published</Table.Th>
                    <Table.Th ta='right'>Downloads</Table.Th>
                    <Table.Th />
                  </Table.Tr>
                </Table.Thead>
                <Table.Tbody>
                  {versions.map((v) => {
                    const action = actionFor(v, installed, !!onModpack);
                    const blocked = !v.file?.url;
                    return (
                      <Table.Tr key={v.id}>
                        <Table.Td>
                          <Group gap={6} wrap='nowrap'>
                            <Badge size='xs' color={versionTypeColor(v.version_type)} variant='light'>
                              {v.version_type}
                            </Badge>
                            <Tooltip label={v.file ? `${v.file.name} · ${formatBytes(v.file.size)}` : v.name}>
                              <Text size='sm' truncate maw={260}>
                                {v.version_number || v.name}
                              </Text>
                            </Tooltip>
                          </Group>
                        </Table.Td>
                        <Table.Td>
                          <Tooltip label={v.game_versions.join(', ')} disabled={v.game_versions.length <= 3}>
                            <Text size='xs' c='dimmed'>
                              {shortVersions(v.game_versions, 3)}
                            </Text>
                          </Tooltip>
                        </Table.Td>
                        <Table.Td>
                          <Text size='xs' c='dimmed'>
                            {formatDate(v.published)}
                          </Text>
                        </Table.Td>
                        <Table.Td ta='right'>
                          <Text size='xs' c='dimmed'>
                            {formatNumber(v.downloads)}
                          </Text>
                        </Table.Td>
                        <Table.Td ta='right'>
                          {action === null ? (
                            <Badge color='green' variant='light' leftSection={<FontAwesomeIcon icon={faCheck} />}>
                              Installed
                            </Badge>
                          ) : blocked ? (
                            <Tooltip label='The author blocks third-party downloads'>
                              <Button
                                size='compact-xs'
                                variant='default'
                                component='a'
                                href={project?.url}
                                target='_blank'
                              >
                                Manual
                              </Button>
                            </Tooltip>
                          ) : (
                            canManage && (
                              <Button
                                size='compact-xs'
                                color={action.color}
                                leftSection={<FontAwesomeIcon icon={action.icon} />}
                                loading={busy === v.id}
                                disabled={busy !== null && busy !== v.id}
                                onClick={() => run(v)}
                              >
                                {action.label}
                              </Button>
                            )
                          )}
                        </Table.Td>
                      </Table.Tr>
                    );
                  })}
                </Table.Tbody>
              </Table>
            </ScrollArea.Autosize>
          )}
        </Tabs.Panel>

        <Tabs.Panel value='details'>
          <ScrollArea.Autosize mah={460}>
            {project?.body ? (
              <Typography>
                <ReactMarkdown>{project.body}</ReactMarkdown>
              </Typography>
            ) : (
              <Text size='sm' c='dimmed'>
                {project ? (
                  <>
                    {project.description}{' '}
                    <Anchor href={project.url} target='_blank' rel='noreferrer'>
                      Full description on {PROVIDER_LABELS[provider]}
                    </Anchor>
                  </>
                ) : (
                  'Loading…'
                )}
              </Text>
            )}
            {project && project.categories.length > 0 && (
              <Group gap={4} mt='md'>
                {project.categories.map((c) => (
                  <Badge key={c} size='xs' variant='default'>
                    {c}
                  </Badge>
                ))}
              </Group>
            )}
          </ScrollArea.Autosize>
        </Tabs.Panel>
      </Tabs>

      <Group justify='flex-end' mt='md'>
        <Button variant='default' onClick={onClose}>
          Close
        </Button>
      </Group>
    </Modal>
  );
}
