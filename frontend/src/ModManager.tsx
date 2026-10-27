import { faInfoCircle, faPuzzlePiece, faSearch, faStethoscope } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { Alert, Button, Group, Loader, Select, Text, Tooltip } from '@mantine/core';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import ServerContentContainer from '@/elements/containers/ServerContentContainer.tsx';
import { useToast } from '@/providers/ToastProvider.tsx';
import { useServerStore } from '@/stores/server.ts';
import * as api from './api';
import Browse from './Browse';
import DiagnoseView from './DiagnoseView';
import InstalledView from './InstalledView';
import ModpackDialog from './ModpackDialog';
import ModpackView from './ModpackView';
import ProjectModal from './ProjectModal';
import {
  ContentKind,
  Context,
  CurrentModpack,
  Installed,
  InstalledList,
  InstallResult,
  ProviderId,
  Settings,
  Target,
  Version,
} from './types';
import { applyHint, isPluginLoader, LOADER_LABELS, PLUGIN_LOADERS, projectKey } from './util';

type View = 'browse' | 'installed' | 'diagnose';

const NOUNS: Record<ContentKind, string> = {
  mods: 'Mods',
  plugins: 'Plugins',
  datapack: 'Data Packs',
  modpack: 'Modpacks',
};

/** The loader to use when switching content: keep it if it fits, else the detected one, else a default. */
const loaderFor = (kind: ContentKind, current: string, detected: string | null) => {
  const fits = (l: string | null) => !!l && l !== 'any' && (kind === 'plugins') === isPluginLoader(l);
  if (kind === 'datapack') return current;
  if (fits(current)) return current;
  if (fits(detected)) return detected!;
  return kind === 'plugins' ? 'paper' : 'fabric';
};

/** A current modpack, shaped like an installed jar, so the version list can mark it. */
const modpackAsInstalled = (m: CurrentModpack): Installed => ({
  file: '',
  size: 0,
  modified: 0,
  enabled: true,
  sha1: null,
  provider: m.provider,
  project: m.project,
  version: m.installed_version,
  update: m.update,
});

const uiKey = (server: string) => `modmanager:ui:${server}`;

/** The content type and tab last open on a server (session storage; empty when unavailable). */
const loadUi = (server: string): { kind?: ContentKind; view?: View } => {
  try {
    return JSON.parse(sessionStorage.getItem(uiKey(server)) ?? '{}');
  } catch {
    return {};
  }
};

const saveUi = (server: string, ui: { kind: ContentKind; view: View }) => {
  try {
    sessionStorage.setItem(uiKey(server), JSON.stringify(ui));
  } catch {
    // private window or storage blocked: the page just starts on its default tab
  }
};

export default function ModManager() {
  const server = useServerStore((state) => state.server.uuid);
  const { addToast } = useToast();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [context, setContext] = useState<Context | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [view, setView] = useState<View>('browse');
  const [provider, setProvider] = useState<ProviderId>('modrinth');
  const [target, setTarget] = useState<Target | null>(null);
  const [installed, setInstalled] = useState<InstalledList | null>(null);
  const [installedLoading, setInstalledLoading] = useState(false);
  const [modpack, setModpack] = useState<CurrentModpack | null>(null);
  const [open, setOpen] = useState<{
    provider: ProviderId;
    id: string;
    tab: 'versions' | 'details';
    /** Diagnose can point at another content type than the one browsed (e.g. a data pack). */
    kind?: ContentKind;
  } | null>(null);
  const [openInstalled, setOpenInstalled] = useState<Installed | null>(null);
  const [searchSeed, setSearchSeed] = useState<{ query: string; n: number } | null>(null);
  const [packDialog, setPackDialog] = useState<{
    provider: ProviderId;
    projectId: string;
    versionId: string;
    title: string;
  } | null>(null);
  const [jobStarted, setJobStarted] = useState(0);

  useEffect(() => {
    api
      .getSettings(server)
      .then((s) => {
        setSettings(s);
        if (s.providers.length > 0) setProvider(s.providers[0]);
        if (s.egg_allowed && !s.restricted && s.can_view) {
          return Promise.all([api.getContext(server), api.modpackJob(server).catch(() => null)]).then(([c, job]) => {
            setContext(c);
            const loader = c.detected.loader;
            // a reinstall during a modpack install reloads this page: go straight back to its progress
            const installing = job?.state === 'running' && Date.now() / 1000 - job.started < 3600;
            const saved = loadUi(server);
            const kind: ContentKind = installing
              ? 'modpack'
              : (saved.kind ??
                // no loader and no mods / plugins folder (e.g. Vanilla): only data packs make sense
                (!loader && !c.detected.directory ? 'datapack' : isPluginLoader(loader) ? 'plugins' : 'mods'));
            setTarget({
              kind,
              loader: loaderFor(kind, loader ?? 'any', loader),
              game_version: c.detected.game_version ?? 'any',
            });
            setView(installing ? 'installed' : (saved.view ?? 'browse'));
          });
        }
      })
      .catch((e) => setError(httpErrorToHuman(e)));
  }, [server]);

  const isModpack = target?.kind === 'modpack';

  // remember the tab per server for this browser session
  useEffect(() => {
    if (target) saveUi(server, { kind: target.kind, view });
  }, [server, target?.kind, view]);

  const loadInstalled = useCallback(
    (fresh = false) => {
      if (!target) return;
      if (target.kind === 'modpack') {
        api
          .modpackCurrent(server, target)
          .then(setModpack)
          .catch(() => setModpack(null));
        return;
      }
      setInstalledLoading(true);
      api
        .getInstalled(server, target, fresh)
        .then(setInstalled)
        .catch((e) => addToast(httpErrorToHuman(e), 'error'))
        .finally(() => setInstalledLoading(false));
    },
    [server, target],
  );

  useEffect(() => loadInstalled(false), [loadInstalled]);

  const installedByProject = useMemo(() => {
    const map = new Map<string, Installed>();
    if (isModpack) {
      if (modpack) map.set(projectKey(modpack.provider, modpack.project_id), modpackAsInstalled(modpack));
      return map;
    }
    for (const m of installed?.mods ?? []) {
      if (m.provider && m.project) map.set(projectKey(m.provider, m.project.id), m);
    }
    return map;
  }, [installed, modpack, isModpack]);

  const onInstalled = (result: InstallResult) => {
    const main = result.installed.filter((i) => !i.required_by);
    const deps = result.installed.filter((i) => i.required_by);
    if (result.installed.length > 0) {
      const what = main.map((i) => `${i.title} ${i.version}`).join(', ') || 'Dependencies';
      const verb = main.some((i) => i.replaced) ? 'Switched to' : 'Installed';
      addToast(
        `${verb} ${what}${deps.length ? ` (+ ${deps.map((d) => d.title).join(', ')})` : ''}. ${applyHint(target)}`,
        'success',
      );
    }
    for (const w of result.warnings) addToast(w, 'warning');
    loadInstalled(false);
  };

  // the installed copy of a project opened for another content type (Diagnose → data pack)
  useEffect(() => {
    setOpenInstalled(null);
    if (!open?.kind || !target || open.kind === target.kind) return;
    api
      .getInstalled(server, { ...target, kind: open.kind })
      .then((l) =>
        setOpenInstalled(l.mods.find((m) => m.provider === open.provider && m.project?.id === open.id) ?? null),
      )
      .catch(() => setOpenInstalled(null));
  }, [open]);

  const openPack = (p: ProviderId, projectId: string, version: Version, title: string) =>
    setPackDialog({ provider: p, projectId, versionId: version.id, title });

  const kind = target?.kind ?? 'mods';
  const noun = NOUNS[kind];
  const detected = context?.detected;
  const familyLoaders = (context?.loaders ?? []).filter((l) =>
    kind === 'plugins' ? PLUGIN_LOADERS.includes(l) : !PLUGIN_LOADERS.includes(l),
  );
  const familyMismatch =
    !!detected?.loader &&
    (kind === 'mods' || kind === 'plugins' || kind === 'modpack') &&
    (kind === 'plugins') !== isPluginLoader(detected.loader);

  const filters = target && context && (
    <>
      <Select
        label='Content'
        w={130}
        allowDeselect={false}
        value={target.kind}
        onChange={(v) => {
          if (!v) return;
          const next = v as ContentKind;
          setTarget({ ...target, kind: next, loader: loaderFor(next, target.loader, detected?.loader ?? null) });
        }}
        data={(['mods', 'plugins', 'datapack', 'modpack'] as ContentKind[]).map((k) => ({ value: k, label: NOUNS[k] }))}
      />
      {kind !== 'datapack' && (
        <Select
          label={
            <Group gap={4} component='span'>
              Game Loader
              {detected && detected.notes.length > 0 && (
                <Tooltip label={detected.notes.join(' ')} multiline w={260}>
                  <FontAwesomeIcon icon={faInfoCircle} style={{ opacity: 0.6 }} />
                </Tooltip>
              )}
            </Group>
          }
          w={150}
          allowDeselect={false}
          value={target.loader}
          onChange={(v) => v && setTarget({ ...target, loader: v })}
          data={[
            { value: 'any', label: 'Any' },
            ...familyLoaders.map((l) => ({
              value: l,
              label: `${LOADER_LABELS[l] ?? l}${l === detected?.loader ? ' (detected)' : ''}`,
            })),
          ]}
        />
      )}
      <Select
        label='Minecraft Version'
        w={170}
        searchable
        allowDeselect={false}
        value={target.game_version}
        onChange={(v) => v && setTarget({ ...target, game_version: v })}
        data={[
          { value: 'any', label: 'All' },
          ...context.game_versions.map((v) => ({
            value: v,
            label: `${v}${v === detected?.game_version ? ' (detected)' : ''}`,
          })),
        ]}
      />
    </>
  );

  const notice = (text: string) => <Text c='dimmed'>{text}</Text>;
  const updates = isModpack ? (modpack?.update ? 1 : 0) : (installed?.mods.filter((m) => m.update).length ?? 0);

  const header = settings?.can_view && target && (
    <Group gap='xs'>
      <Button
        variant={view === 'browse' ? 'filled' : 'default'}
        leftSection={<FontAwesomeIcon icon={faSearch} />}
        onClick={() => setView('browse')}
      >
        Browse {noun}
      </Button>
      <Button
        variant={view === 'installed' ? 'filled' : 'default'}
        leftSection={<FontAwesomeIcon icon={faPuzzlePiece} />}
        onClick={() => setView('installed')}
      >
        Manage {noun}
        {updates > 0 ? ` (${updates})` : ''}
      </Button>
      <Button
        variant={view === 'diagnose' ? 'filled' : 'default'}
        leftSection={<FontAwesomeIcon icon={faStethoscope} />}
        onClick={() => setView('diagnose')}
      >
        Diagnose
      </Button>
    </Group>
  );

  return (
    <ServerContentContainer title={`Minecraft ${noun}`} contentRight={header || undefined}>
      {error ? (
        <Alert color='red'>{error}</Alert>
      ) : settings === null ? (
        <Group justify='center' py='xl'>
          <Loader />
        </Group>
      ) : settings.restricted ? (
        notice('Mod Manager is not available to your account on this panel. Ask a panel administrator for access.')
      ) : !settings.egg_allowed ? (
        notice(
          'Mod Manager is not enabled for this server type. An administrator can enable it per egg under Admin → Extensions → Mod Manager.',
        )
      ) : !settings.can_view ? (
        notice('You do not have permission to view mods on this server.')
      ) : settings.providers.length === 0 ? (
        notice(
          'No mod source is enabled on this panel. An administrator can enable Modrinth or CurseForge under Admin → Extensions → Mod Manager.',
        )
      ) : !target ? (
        <Group justify='center' py='xl'>
          <Loader />
        </Group>
      ) : (
        <>
          {!detected?.loader && (kind === 'mods' || kind === 'plugins') && (
            <Alert color='yellow' mb='md'>
              Could not detect which loader this server runs. Pick the Game Loader so {noun.toLowerCase()} land in the
              right folder.
            </Alert>
          )}
          {familyMismatch && (
            <Alert color='yellow' mb='md'>
              This server runs {LOADER_LABELS[detected!.loader!] ?? detected!.loader}:{' '}
              {kind === 'plugins'
                ? 'plugins will be installed into /plugins, but this server does not load them.'
                : `${noun.toLowerCase()} for mod loaders do not run on it.`}
            </Alert>
          )}
          {view === 'diagnose' ? (
            <DiagnoseView
              server={server}
              settings={settings}
              target={target}
              onOpen={(p, id, tab, k) => setOpen({ provider: p, id, tab, kind: k })}
              onInstalled={onInstalled}
              onSearch={(k, query) => {
                setTarget({ ...target, kind: k, loader: loaderFor(k, target.loader, detected?.loader ?? null) });
                setSearchSeed((s) => ({ query, n: (s?.n ?? 0) + 1 }));
                setView('browse');
              }}
            />
          ) : view === 'browse' ? (
            <Browse
              searchSeed={searchSeed}
              server={server}
              settings={settings}
              target={target}
              filters={filters}
              provider={provider}
              setProvider={setProvider}
              installedByProject={installedByProject}
              onOpen={(p, id, tab) => setOpen({ provider: p, id, tab })}
              onInstalled={onInstalled}
            />
          ) : isModpack ? (
            <ModpackView
              server={server}
              settings={settings}
              target={target}
              filters={filters}
              jobStarted={jobStarted}
              onOpen={(p, id, tab) => setOpen({ provider: p, id, tab })}
              onUpdate={(m) => m.update && openPack(m.provider, m.project_id, m.update, m.title)}
              onForgotten={() => setModpack(null)}
              onReinstall={(m) =>
                setPackDialog({
                  provider: m.provider,
                  projectId: m.project_id,
                  versionId: m.version_id,
                  title: m.title,
                })
              }
            />
          ) : (
            <InstalledView
              server={server}
              settings={settings}
              target={target}
              filters={filters}
              list={installed}
              loading={installedLoading}
              onRefresh={loadInstalled}
              onOpen={(p, id, tab) => setOpen({ provider: p, id, tab })}
              onInstalled={onInstalled}
            />
          )}
        </>
      )}

      {open && target && settings && (
        <ProjectModal
          server={server}
          target={open.kind && open.kind !== target.kind ? { ...target, kind: open.kind } : target}
          provider={open.provider}
          projectId={open.id}
          installed={
            open.kind && open.kind !== target.kind
              ? openInstalled
              : (installedByProject.get(projectKey(open.provider, open.id)) ?? null)
          }
          canManage={settings.can_manage}
          initialTab={open.tab}
          onClose={() => setOpen(null)}
          onInstalled={onInstalled}
          onModpack={isModpack ? (v, title) => openPack(open.provider, open.id, v, title) : undefined}
        />
      )}

      {packDialog && (
        <ModpackDialog
          server={server}
          provider={packDialog.provider}
          projectId={packDialog.projectId}
          versionId={packDialog.versionId}
          title={packDialog.title}
          replacing={!!modpack}
          onClose={() => setPackDialog(null)}
          onStarted={() => {
            setView('installed');
            setJobStarted((n) => n + 1);
            addToast(`Installing ${packDialog.title}. Follow the progress here.`, 'success');
          }}
        />
      )}
    </ServerContentContainer>
  );
}
