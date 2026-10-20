import { faFileExport, faFileImport } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import {
  Alert,
  Badge,
  Button,
  Checkbox,
  Group,
  Loader,
  Modal,
  Progress,
  ScrollArea,
  Stack,
  Table,
  Text,
  Tooltip,
} from '@mantine/core';
import { useRef, useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import { useToast } from '@/providers/ToastProvider.tsx';
import { useServerStore } from '@/stores/server.ts';
import * as api from './api';
import { ImportPreview, ImportRow, InstallResult, ModList, Settings, Target } from './types';
import { loaderLabel, PROVIDER_LABELS, versionTypeColor } from './util';
import { PackContents, readPack } from './zip';

interface Props {
  server: string;
  settings: Settings;
  target: Target;
  onInstalled: (result: InstallResult) => void;
}

const STATUS: Record<ImportRow['status'], { label: string; color: string }> = {
  ready: { label: 'Ready', color: 'blue' },
  installed: { label: 'Installed', color: 'green' },
  incompatible: { label: 'No version', color: 'orange' },
  blocked: { label: 'Manual', color: 'yellow' },
  error: { label: 'Error', color: 'red' },
};

const describe = (loader: string | null, version: string | null) =>
  `${loaderLabel(loader)}${version ? ` ${version}` : ''}`;

/** Export the installed mods as a JSON list, or import such a list onto this server. */
export default function ModListTools({ server, settings, target, onInstalled }: Props) {
  const { addToast } = useToast();
  const serverName = useServerStore((state) => state.server.name);
  const fileInput = useRef<HTMLInputElement>(null);
  const [exporting, setExporting] = useState(false);
  const [readingPack, setReadingPack] = useState(false);
  const [list, setList] = useState<ModList | null>(null);
  const [preview, setPreview] = useState<ImportPreview | null>(null);
  const [previewing, setPreviewing] = useState(false);
  const [exact, setExact] = useState(false);
  const [withDependencies, setWithDependencies] = useState(true);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [progress, setProgress] = useState<{ done: number; total: number; current: string } | null>(null);

  const doExport = async () => {
    setExporting(true);
    try {
      const data = await api.exportList(server, target);
      const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
      const slug = (serverName || 'server').replace(/[^a-z0-9]+/gi, '-').toLowerCase();
      const name = `${slug}-mods-${data.loader ?? 'any'}-${data.game_version ?? 'any'}.json`;
      const a = document.createElement('a');
      a.href = URL.createObjectURL(blob);
      a.download = name;
      a.click();
      URL.revokeObjectURL(a.href);
      addToast(
        `Exported ${data.mods.length} mods${data.unrecognised.length ? ` (${data.unrecognised.length} unrecognised jars listed separately)` : ''}.`,
        'success',
      );
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
    } finally {
      setExporting(false);
    }
  };

  const loadPreview = async (l: ModList, useExact: boolean) => {
    setPreviewing(true);
    try {
      const p = await api.previewImport(server, target, l, useExact);
      setPreview(p);
      // ready mods that were enabled on the source server
      setSelected(new Set(p.rows.flatMap((r, i) => (r.status === 'ready' && r.entry.enabled ? [i] : []))));
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
      setList(null);
    } finally {
      setPreviewing(false);
    }
  };

  const open = (parsed: ModList) => {
    setList(parsed);
    setExact(false);
    setPreview(null);
    loadPreview(parsed, false);
  };

  const onFile = async (file: File | undefined) => {
    if (!file) return;
    if (file.name.toLowerCase().endsWith('.json')) {
      try {
        const parsed = JSON.parse(await file.text()) as ModList;
        if (!parsed || !Array.isArray(parsed.mods)) throw new Error('not a mod list');
        open(parsed);
      } catch {
        addToast('That file is not a Mod Manager mod list (export one from the Manage tab).', 'error');
      }
      return;
    }
    // a launcher modpack: Modrinth .mrpack or CurseForge .zip (Prism Launcher → Export)
    setReadingPack(true);
    try {
      let pack: PackContents;
      try {
        pack = await readPack(file);
      } catch (e) {
        throw new Error(`Could not read the pack: ${e instanceof Error ? e.message : String(e)}`);
      }
      open(await api.importPack(server, pack));
    } catch (e) {
      addToast(e instanceof Error && !('isAxiosError' in e) ? e.message : httpErrorToHuman(e), 'error');
    } finally {
      setReadingPack(false);
    }
  };

  const close = () => {
    if (progress) return;
    setList(null);
    setPreview(null);
  };

  const run = async () => {
    if (!preview) return;
    const rows = preview.rows.filter((_, i) => selected.has(i));
    const merged: InstallResult = { installed: [], warnings: [] };
    for (const [i, row] of rows.entries()) {
      setProgress({ done: i, total: rows.length, current: row.entry.title });
      try {
        const r = await api.install(server, target, {
          provider: row.entry.provider,
          project_id: row.entry.project_id,
          version_id: row.version?.id ?? null,
          dependencies: withDependencies,
        });
        merged.installed.push(...r.installed);
        merged.warnings.push(...r.warnings);
      } catch (e) {
        merged.warnings.push(`${row.entry.title}: ${httpErrorToHuman(e)}`);
      }
    }
    setProgress(null);
    setList(null);
    setPreview(null);
    onInstalled(merged);
  };

  const toggle = (i: number) => {
    const next = new Set(selected);
    if (next.has(i)) next.delete(i);
    else next.add(i);
    setSelected(next);
  };

  const readyIdx = preview?.rows.flatMap((r, i) => (r.status === 'ready' ? [i] : [])) ?? [];
  const counts = (s: ImportRow['status']) => preview?.rows.filter((r) => r.status === s).length ?? 0;

  return (
    <>
      <Button
        variant='default'
        size='xs'
        leftSection={<FontAwesomeIcon icon={faFileExport} />}
        loading={exporting}
        onClick={doExport}
      >
        Export list
      </Button>
      {settings.can_manage && (
        <Tooltip
          label='A mod list exported here (.json), or a modpack exported from Prism Launcher, Modrinth (.mrpack) or CurseForge (.zip)'
          multiline
          w={280}
        >
          <Button
            variant='default'
            size='xs'
            leftSection={<FontAwesomeIcon icon={faFileImport} />}
            loading={readingPack}
            onClick={() => fileInput.current?.click()}
          >
            Import list
          </Button>
        </Tooltip>
      )}
      <input
        ref={fileInput}
        type='file'
        accept='application/json,.json,.mrpack,.zip'
        style={{ display: 'none' }}
        onChange={(e) => {
          onFile(e.currentTarget.files?.[0]);
          e.currentTarget.value = '';
        }}
      />

      <Modal opened={list !== null} onClose={close} size='xl' centered title='Import mod list'>
        {list && (
          <Stack>
            <Text size='sm'>
              {list.mods.length} mods from <b>{list.server ?? 'another server'}</b> (
              {describe(list.loader, list.game_version)}) onto this server (
              {describe(
                target.loader === 'any' ? null : target.loader,
                target.game_version === 'any' ? null : target.game_version,
              )}
              ).
            </Text>
            <Group gap='md'>
              <Tooltip
                label='Off: the newest version for this server’s loader and Minecraft version. On: exactly the versions in the list (only sensible for the same Minecraft version).'
                multiline
                w={300}
              >
                <Checkbox
                  size='xs'
                  label='Use the exact exported versions'
                  checked={exact}
                  disabled={previewing || progress !== null}
                  onChange={(e) => {
                    setExact(e.currentTarget.checked);
                    loadPreview(list, e.currentTarget.checked);
                  }}
                />
              </Tooltip>
              <Checkbox
                size='xs'
                label='Install required dependencies'
                checked={withDependencies}
                disabled={progress !== null}
                onChange={(e) => setWithDependencies(e.currentTarget.checked)}
              />
            </Group>

            {previewing || !preview ? (
              <Group justify='center' py='xl' gap='sm'>
                <Loader size='sm' />
                <Text size='sm' c='dimmed'>
                  Finding versions for {list.mods.length} mods…
                </Text>
              </Group>
            ) : (
              <>
                <Group gap='xs'>
                  {(['ready', 'installed', 'incompatible', 'blocked', 'error'] as const).map(
                    (s) =>
                      counts(s) > 0 && (
                        <Badge key={s} color={STATUS[s].color} variant='light'>
                          {counts(s)} {STATUS[s].label.toLowerCase()}
                        </Badge>
                      ),
                  )}
                </Group>
                <ScrollArea.Autosize mah={420}>
                  <Table verticalSpacing={6} striped>
                    <Table.Thead>
                      <Table.Tr>
                        <Table.Th w={36}>
                          <Checkbox
                            size='xs'
                            aria-label='Select all'
                            checked={readyIdx.length > 0 && readyIdx.every((i) => selected.has(i))}
                            indeterminate={selected.size > 0 && !readyIdx.every((i) => selected.has(i))}
                            disabled={progress !== null}
                            onChange={(e) => setSelected(new Set(e.currentTarget.checked ? readyIdx : []))}
                          />
                        </Table.Th>
                        <Table.Th>Mod</Table.Th>
                        <Table.Th>Exported</Table.Th>
                        <Table.Th>Will install</Table.Th>
                        <Table.Th>Status</Table.Th>
                      </Table.Tr>
                    </Table.Thead>
                    <Table.Tbody>
                      {preview.rows.map((r, i) => (
                        <Table.Tr key={`${r.entry.provider}:${r.entry.project_id}`}>
                          <Table.Td>
                            <Checkbox
                              size='xs'
                              aria-label={`Install ${r.entry.title}`}
                              checked={selected.has(i)}
                              disabled={r.status !== 'ready' || progress !== null}
                              onChange={() => toggle(i)}
                            />
                          </Table.Td>
                          <Table.Td>
                            <Group gap={6} wrap='nowrap'>
                              <Text size='sm' truncate maw={220}>
                                {r.entry.title || r.entry.slug || r.entry.project_id}
                              </Text>
                              <Badge size='xs' variant='default'>
                                {PROVIDER_LABELS[r.entry.provider] ?? r.entry.provider}
                              </Badge>
                              {!r.entry.enabled && (
                                <Badge size='xs' variant='default'>
                                  was disabled
                                </Badge>
                              )}
                            </Group>
                          </Table.Td>
                          <Table.Td>
                            <Text size='xs' c='dimmed' truncate maw={140}>
                              {r.entry.version_number ?? '—'}
                            </Text>
                          </Table.Td>
                          <Table.Td>
                            {r.version ? (
                              <Group gap={4} wrap='nowrap'>
                                <Text size='xs' truncate maw={160}>
                                  {r.version.version_number}
                                </Text>
                                {r.version.version_type !== 'release' && (
                                  <Badge size='xs' variant='light' color={versionTypeColor(r.version.version_type)}>
                                    {r.version.version_type}
                                  </Badge>
                                )}
                              </Group>
                            ) : (
                              <Text size='xs' c='dimmed'>
                                —
                              </Text>
                            )}
                          </Table.Td>
                          <Table.Td>
                            <Tooltip label={r.message} disabled={!r.message} multiline w={280}>
                              <Badge size='sm' variant='light' color={STATUS[r.status].color}>
                                {STATUS[r.status].label}
                              </Badge>
                            </Tooltip>
                          </Table.Td>
                        </Table.Tr>
                      ))}
                    </Table.Tbody>
                  </Table>
                </ScrollArea.Autosize>
                {preview.skipped.length > 0 && (
                  <Alert color='gray' py='xs'>
                    <Text size='xs'>
                      Left out {preview.skipped.length} client-only mod{preview.skipped.length === 1 ? '' : 's'} (they
                      do not run on servers): {preview.skipped.join(', ')}
                    </Text>
                  </Alert>
                )}
                {preview.unrecognised.length > 0 && (
                  <Alert color='gray' py='xs'>
                    <Text size='xs'>
                      Not on Modrinth or CurseForge, copy these by hand if you need them:{' '}
                      {preview.unrecognised.join(', ')}
                    </Text>
                  </Alert>
                )}
              </>
            )}

            {progress && (
              <Stack gap={4}>
                <Text size='xs' c='dimmed'>
                  Installing {progress.current} ({progress.done + 1} of {progress.total})…
                </Text>
                <Progress value={(progress.done / progress.total) * 100} animated />
              </Stack>
            )}

            <Group justify='flex-end'>
              <Button variant='default' onClick={close} disabled={progress !== null}>
                Cancel
              </Button>
              <Button
                onClick={run}
                loading={progress !== null}
                disabled={!preview || selected.size === 0 || previewing}
              >
                Install {selected.size} mod{selected.size === 1 ? '' : 's'}
              </Button>
            </Group>
          </Stack>
        )}
      </Modal>
    </>
  );
}
