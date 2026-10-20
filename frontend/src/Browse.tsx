import {
  faArrowUp,
  faCheck,
  faDownload,
  faExternalLinkAlt,
  faHeart,
  faInfoCircle,
  faSearch,
  faUser,
} from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import {
  ActionIcon,
  Alert,
  Badge,
  Button,
  Checkbox,
  Group,
  Loader,
  Pagination,
  Paper,
  Select,
  SimpleGrid,
  Stack,
  Text,
  TextInput,
  Tooltip,
} from '@mantine/core';
import { useDebouncedValue } from '@mantine/hooks';
import { useEffect, useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import { useToast } from '@/providers/ToastProvider.tsx';
import * as api from './api';
import ProjectIcon from './ProjectIcon';
import { Installed, InstallResult, Project, ProviderId, SearchPage, Settings, Target } from './types';
import { formatNumber, PROVIDER_LABELS, projectKey, SORTS } from './util';

interface Props {
  /** Set from Diagnose: search for this. */
  searchSeed?: { query: string; n: number } | null;
  server: string;
  settings: Settings;
  target: Target;
  filters: React.ReactNode;
  provider: ProviderId;
  setProvider: (p: ProviderId) => void;
  installedByProject: Map<string, Installed>;
  onOpen: (provider: ProviderId, id: string, tab: 'versions' | 'details') => void;
  onInstalled: (result: InstallResult) => void;
}

const PER_PAGE = ['12', '24', '48'];

function Card({
  project,
  installed,
  canManage,
  busy,
  onInstall,
  onOpen,
}: {
  project: Project;
  installed: Installed | undefined;
  canManage: boolean;
  busy: boolean;
  onInstall: () => void;
  onOpen: (tab: 'versions' | 'details') => void;
}) {
  return (
    <Paper withBorder radius='md' p='md' style={{ display: 'flex', flexDirection: 'column' }}>
      <Group align='flex-start' wrap='nowrap' gap='sm'>
        <ProjectIcon url={project.icon_url} title={project.title} />
        <Stack gap={2} style={{ minWidth: 0, flex: 1 }}>
          <Group gap={6} wrap='nowrap'>
            <Text fw={500} truncate title={project.title}>
              {project.title}
            </Text>
            {project.server_side === 'unsupported' && (
              <Tooltip label='Marked as client-only: it does nothing on a server'>
                <Badge size='xs' color='orange' variant='light' style={{ flexShrink: 0 }}>
                  Client-only
                </Badge>
              </Tooltip>
            )}
          </Group>
          <Group gap='sm'>
            <Text size='xs' c='dimmed'>
              <FontAwesomeIcon icon={faDownload} /> {formatNumber(project.downloads)}
            </Text>
            <Text size='xs' c='dimmed'>
              <FontAwesomeIcon icon={faHeart} /> {formatNumber(project.follows)}
            </Text>
          </Group>
          {project.author && (
            <Text size='xs' c='dimmed' truncate>
              <FontAwesomeIcon icon={faUser} /> by {project.author}
            </Text>
          )}
        </Stack>
      </Group>
      <Text size='sm' mt='sm' lineClamp={2} style={{ flex: 1 }} title={project.description}>
        {project.description}
      </Text>
      <Group justify='space-between' mt='md' wrap='nowrap'>
        <Tooltip label={`Open on ${PROVIDER_LABELS[project.provider]}`}>
          <ActionIcon variant='default' size='lg' component='a' href={project.url} target='_blank' rel='noreferrer'>
            <FontAwesomeIcon icon={faExternalLinkAlt} />
          </ActionIcon>
        </Tooltip>
        <Group gap='xs' wrap='nowrap'>
          <Button
            variant='default'
            leftSection={<FontAwesomeIcon icon={faInfoCircle} />}
            onClick={() => onOpen('details')}
          >
            Details
          </Button>
          {installed ? (
            installed.update ? (
              <Button
                color='green'
                leftSection={<FontAwesomeIcon icon={faArrowUp} />}
                onClick={() => onOpen('versions')}
                disabled={!canManage}
              >
                Update
              </Button>
            ) : (
              <Tooltip label={`${installed.version?.version_number ?? installed.file} — change version`}>
                <Button
                  variant='light'
                  color='green'
                  leftSection={<FontAwesomeIcon icon={faCheck} />}
                  onClick={() => onOpen('versions')}
                >
                  Installed
                </Button>
              </Tooltip>
            )
          ) : (
            canManage && (
              <Button leftSection={<FontAwesomeIcon icon={faDownload} />} loading={busy} onClick={onInstall}>
                Install
              </Button>
            )
          )}
        </Group>
      </Group>
    </Paper>
  );
}

export default function Browse({
  searchSeed,
  server,
  settings,
  target,
  filters,
  provider,
  setProvider,
  installedByProject,
  onOpen,
  onInstalled,
}: Props) {
  const { addToast } = useToast();
  const [sort, setSort] = useState('relevance');
  const [perPage, setPerPage] = useState('24');
  const [serverOnly, setServerOnly] = useState(true);
  // CurseForge does not say which projects run on servers
  const serverFilter = provider === 'modrinth';
  const [query, setQuery] = useState(searchSeed?.query ?? '');
  useEffect(() => {
    if (searchSeed) setQuery(searchSeed.query);
  }, [searchSeed]);
  const [debounced] = useDebouncedValue(query, 400);
  const [page, setPage] = useState(1);
  const [result, setResult] = useState<SearchPage | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [installing, setInstalling] = useState<string | null>(null);

  // back to page 1 whenever the filters change
  useEffect(
    () => setPage(1),
    [provider, sort, perPage, debounced, serverOnly, target.kind, target.loader, target.game_version],
  );

  useEffect(() => {
    let alive = true;
    setLoading(true);
    setError(null);
    api
      .search(server, {
        provider,
        query: debounced,
        sort,
        page,
        per_page: Number(perPage),
        server_only: serverFilter && serverOnly,
        ...target,
      })
      .then((r) => alive && setResult(r))
      .catch((e) => alive && setError(httpErrorToHuman(e)))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [server, provider, sort, perPage, debounced, page, serverOnly, target.kind, target.loader, target.game_version]);

  const quickInstall = async (p: Project) => {
    setInstalling(p.id);
    try {
      onInstalled(await api.install(server, target, { provider: p.provider, project_id: p.id, dependencies: true }));
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
    } finally {
      setInstalling(null);
    }
  };

  const total = result?.total ?? 0;
  const pages = Math.max(1, Math.ceil(total / Number(perPage)));
  const from = total === 0 ? 0 : (page - 1) * Number(perPage) + 1;
  const to = Math.min(total, page * Number(perPage));

  return (
    <Stack>
      <Group align='flex-end' gap='sm' wrap='wrap'>
        <Select
          label='Provider'
          w={150}
          allowDeselect={false}
          value={provider}
          onChange={(v) => v && setProvider(v as ProviderId)}
          data={(['modrinth', 'curseforge'] as ProviderId[]).map((p) => ({
            value: p,
            label: PROVIDER_LABELS[p],
            disabled: !settings.providers.includes(p),
          }))}
        />
        <Select
          label='Sort by'
          w={160}
          allowDeselect={false}
          value={sort}
          onChange={(v) => v && setSort(v)}
          data={SORTS}
        />
        {filters}
        <Select
          label='Results'
          w={90}
          allowDeselect={false}
          value={perPage}
          onChange={(v) => v && setPerPage(v)}
          data={PER_PAGE}
        />
        <TextInput
          label='Search'
          style={{ flex: 1, minWidth: 200 }}
          placeholder={`Search ${PROVIDER_LABELS[provider]}…`}
          leftSection={<FontAwesomeIcon icon={faSearch} />}
          value={query}
          onChange={(e) => setQuery(e.currentTarget.value)}
        />
      </Group>

      {!settings.providers.includes('curseforge') && provider === 'modrinth' && settings.curseforge_disabled_reason && (
        <Text size='xs' c='dimmed'>
          {settings.curseforge_disabled_reason}
        </Text>
      )}

      <Group justify='space-between'>
        <Group gap='lg'>
          <Text size='sm' c='dimmed'>
            {loading && !result
              ? 'Searching…'
              : `Showing ${formatNumber(from)} to ${formatNumber(to)} of ${formatNumber(total)} results.`}
          </Text>
          <Tooltip
            label={
              serverFilter
                ? 'Hide projects marked as client-only (shaders, HUDs, minimaps …): they do nothing on a server.'
                : 'CurseForge does not say which projects run on servers, so this filter only works for Modrinth.'
            }
            multiline
            w={280}
          >
            <Checkbox
              size='xs'
              label='Server-side only'
              checked={serverFilter && serverOnly}
              disabled={!serverFilter}
              onChange={(e) => setServerOnly(e.currentTarget.checked)}
            />
          </Tooltip>
        </Group>
        <Group gap='sm'>
          {loading && <Loader size='xs' />}
          <Pagination size='sm' total={pages} value={page} onChange={setPage} siblings={1} boundaries={1} />
        </Group>
      </Group>

      {error && <Alert color='red'>{error}</Alert>}

      <SimpleGrid cols={{ base: 1, sm: 2, lg: 3, xl: 4 }} style={{ opacity: loading ? 0.6 : 1 }}>
        {(result?.hits ?? []).map((p) => (
          <Card
            key={projectKey(p.provider, p.id)}
            project={p}
            installed={installedByProject.get(projectKey(p.provider, p.id))}
            canManage={settings.can_manage}
            busy={installing === p.id}
            onInstall={() => (target.kind === 'modpack' ? onOpen(p.provider, p.id, 'versions') : quickInstall(p))}
            onOpen={(tab) => onOpen(p.provider, p.id, tab)}
          />
        ))}
      </SimpleGrid>

      {result && result.hits.length === 0 && !loading && (
        <Text c='dimmed' ta='center' py='xl'>
          Nothing found. Try another search, or loosen the loader / Minecraft version filters.
        </Text>
      )}

      {pages > 1 && (
        <Group justify='flex-end'>
          <Pagination size='sm' total={pages} value={page} onChange={setPage} siblings={1} boundaries={1} />
        </Group>
      )}
    </Stack>
  );
}
