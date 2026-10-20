import {
  faBan,
  faCircleCheck,
  faCircleExclamation,
  faCircleInfo,
  faCodeBranch,
  faDownload,
  faSearch,
  faStethoscope,
  faTriangleExclamation,
} from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { Alert, Badge, Button, Code, Collapse, Group, Paper, Select, Stack, Text, Textarea } from '@mantine/core';
import { useEffect, useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import { useToast } from '@/providers/ToastProvider.tsx';
import * as api from './api';
import {
  ContentKind,
  CrashAction,
  CrashAnalysis,
  CrashFinding,
  CrashSource,
  InstallResult,
  ProviderId,
  Settings,
  Target,
} from './types';
import { formatBytes } from './util';

interface Props {
  server: string;
  settings: Settings;
  target: Target;
  onOpen: (provider: ProviderId, id: string, tab: 'versions' | 'details', kind: ContentKind) => void;
  onInstalled: (result: InstallResult) => void;
  onSearch: (kind: ContentKind, query: string) => void;
}

const SEVERITY = {
  error: { color: 'red', icon: faCircleExclamation, label: 'Problem' },
  warning: { color: 'yellow', icon: faTriangleExclamation, label: 'Suspect' },
  info: { color: 'blue', icon: faCircleInfo, label: 'Note' },
} as const;

const actionIcon = (a: CrashAction) =>
  a.type === 'disable' ? faBan : a.type === 'versions' ? faCodeBranch : a.type === 'install' ? faDownload : faSearch;

function ActionButton({
  action: a,
  primary,
  busy,
  done,
  onAction,
}: {
  action: CrashAction;
  primary?: boolean;
  busy: string | null;
  done: Set<string>;
  onAction: (a: CrashAction) => void;
}) {
  const key = JSON.stringify(a);
  const finished = done.has(key);
  return (
    <Button
      size='xs'
      variant={primary && a.type !== 'disable' ? 'filled' : 'default'}
      color={primary && a.type !== 'disable' ? (a.type === 'install' ? 'green' : 'blue') : undefined}
      leftSection={<FontAwesomeIcon icon={finished ? faCircleCheck : actionIcon(a)} />}
      loading={busy === key}
      disabled={finished || (busy !== null && busy !== key)}
      onClick={() => onAction(a)}
    >
      {finished ? (a.type === 'disable' ? 'Disabled' : 'Done') : a.label}
    </Button>
  );
}

function FindingCard({
  finding,
  canManage,
  busy,
  done,
  onAction,
}: {
  finding: CrashFinding;
  canManage: boolean;
  busy: string | null;
  done: Set<string>;
  onAction: (a: CrashAction) => void;
}) {
  const [showLines, setShowLines] = useState(false);
  const [showOthers, setShowOthers] = useState(false);
  const s = SEVERITY[finding.severity];
  return (
    <Paper withBorder radius='md' p='md' style={{ borderLeft: `4px solid var(--mantine-color-${s.color}-6)` }}>
      <Group gap='xs' mb={4}>
        <FontAwesomeIcon icon={s.icon} color={`var(--mantine-color-${s.color}-6)`} />
        <Text fw={600}>{finding.title}</Text>
        <Badge size='xs' color={s.color} variant='light'>
          {s.label}
        </Badge>
      </Group>
      <Text size='sm'>{finding.detail}</Text>
      {finding.evidence.length > 0 && (
        <>
          <Button variant='subtle' size='compact-xs' mt={6} onClick={() => setShowLines((v) => !v)}>
            {showLines ? 'Hide' : 'Show'} log line{finding.evidence.length === 1 ? '' : 's'}
          </Button>
          <Collapse expanded={showLines}>
            <Code block mt={4} style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-all', fontSize: 11 }}>
              {finding.evidence.join('\n')}
            </Code>
          </Collapse>
        </>
      )}
      {canManage && finding.actions.length > 0 && (
        <Group gap='xs' mt='sm'>
          {finding.actions.map((a) => (
            <ActionButton key={JSON.stringify(a)} action={a} primary busy={busy} done={done} onAction={onAction} />
          ))}
          {finding.alternatives.length > 0 && (
            <Button variant='subtle' size='compact-xs' onClick={() => setShowOthers((v) => !v)}>
              {showOthers ? 'Hide other options' : `Other options (${finding.alternatives.length})`}
            </Button>
          )}
        </Group>
      )}
      {canManage && finding.alternatives.length > 0 && (
        <Collapse expanded={showOthers}>
          <Group gap='xs' mt='xs'>
            {finding.alternatives.map((a) => (
              <ActionButton key={JSON.stringify(a)} action={a} busy={busy} done={done} onAction={onAction} />
            ))}
          </Group>
        </Collapse>
      )}
    </Paper>
  );
}

/** Diagnose: read a log or crash report and recommend what to change. */
export default function DiagnoseView({ server, settings, target, onOpen, onInstalled, onSearch }: Props) {
  const { addToast } = useToast();
  const [sources, setSources] = useState<CrashSource[] | null>(null);
  const [sourcesError, setSourcesError] = useState<string | null>(null);
  const [source, setSource] = useState<string | null>(null);
  const [pasting, setPasting] = useState(false);
  const [text, setText] = useState('');
  const [result, setResult] = useState<CrashAnalysis | null>(null);
  const [running, setRunning] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [done, setDone] = useState<Set<string>>(new Set());

  useEffect(() => {
    api
      .crashSources(server)
      .then((list) => {
        setSources(list);
        // the newest of latest.log and the crash reports
        const first = list.find((s) => s.path === 'logs/latest.log' || s.path.startsWith('crash-reports/'));
        setSource(first?.path ?? list[0]?.path ?? null);
        if (list.length === 0) setPasting(true);
      })
      .catch((e) => {
        setSourcesError(httpErrorToHuman(e));
        setSources([]);
        setPasting(true);
      });
  }, [server]);

  const run = async () => {
    setRunning(true);
    setResult(null);
    setDone(new Set());
    try {
      setResult(await api.analyzeCrash(server, pasting ? { text } : { source: source ?? undefined }));
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
    } finally {
      setRunning(false);
    }
  };

  const onAction = async (a: CrashAction) => {
    const key = JSON.stringify(a);
    const t: Target = { ...target, kind: a.kind };
    if (a.type === 'versions') {
      onOpen(a.provider, a.project_id, 'versions', a.kind);
      return;
    }
    if (a.type === 'search') {
      onSearch(a.kind, a.query);
      return;
    }
    setBusy(key);
    try {
      if (a.type === 'disable') {
        await api.toggle(server, t, a.file, false);
        addToast(`${a.label.replace(/^Disable /, '')} disabled. Restart the server to apply.`, 'success');
      } else {
        onInstalled(
          await api.install(server, t, { provider: a.provider, project_id: a.project_id, dependencies: true }),
        );
      }
      setDone((d) => new Set(d).add(key));
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
    } finally {
      setBusy(null);
    }
  };

  const errors = result?.findings.filter((f) => f.severity === 'error').length ?? 0;

  return (
    <Stack>
      <Text size='sm' c='dimmed'>
        Reads a server log or crash report and looks for the usual reasons a modded server fails: missing or wrong
        dependencies, mods for another Minecraft version, client-only mods, mods that clash, broken data packs, plugin
        dependencies, Java version and memory. Nothing is changed until you click an action.
      </Text>

      <Group align='flex-end' gap='sm' wrap='wrap'>
        {!pasting && (
          <Select
            label='Log'
            style={{ flex: 1, minWidth: 260 }}
            allowDeselect={false}
            value={source}
            onChange={setSource}
            placeholder={sources === null ? 'Loading…' : 'No logs found'}
            data={(sources ?? []).map((s) => ({
              value: s.path,
              label: `${s.label} · ${new Date(s.modified * 1000).toLocaleString()} · ${formatBytes(s.size)}`,
            }))}
          />
        )}
        <Button
          leftSection={<FontAwesomeIcon icon={faStethoscope} />}
          loading={running}
          disabled={pasting ? !text.trim() : !source}
          onClick={run}
        >
          {result ? 'Analyse again' : 'Analyse'}
        </Button>
        {(sources?.length ?? 0) > 0 && (
          <Button variant='default' onClick={() => setPasting((p) => !p)}>
            {pasting ? 'Pick a log from the server' : 'Paste a log instead'}
          </Button>
        )}
      </Group>
      {sourcesError && (
        <Text size='xs' c='dimmed'>
          {sourcesError} You can still paste a log.
        </Text>
      )}
      {pasting && (
        <Textarea
          label='Log or crash report'
          description='Paste latest.log or a crash report, for example from a friend’s server.'
          autosize
          minRows={6}
          maxRows={16}
          value={text}
          onChange={(e) => setText(e.currentTarget.value)}
          styles={{ input: { fontFamily: 'monospace', fontSize: 12 } }}
        />
      )}

      {result && (
        <Stack gap='sm'>
          <Alert
            color={result.crashed ? (errors ? 'red' : 'yellow') : result.findings.length ? 'yellow' : 'green'}
            icon={<FontAwesomeIcon icon={result.crashed ? faCircleExclamation : faCircleCheck} />}
            title={result.summary}
          >
            <Text size='xs' c='dimmed'>
              {result.source} · {result.lines.toLocaleString()} lines read
            </Text>
            {result.headline.length > 0 && (
              <Code block mt='xs' style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-all', fontSize: 11 }}>
                {result.headline.join('\n')}
              </Code>
            )}
          </Alert>
          {result.findings.map((f) => (
            <FindingCard
              key={f.title + f.evidence[0]}
              finding={f}
              canManage={settings.can_manage}
              busy={busy}
              done={done}
              onAction={onAction}
            />
          ))}
          {result.findings.length > 0 && (
            <Text size='xs' c='dimmed'>
              After changing mods, restart the server and analyse the new latest.log: fixing one problem can reveal the
              next.
            </Text>
          )}
        </Stack>
      )}
    </Stack>
  );
}
