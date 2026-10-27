import { faCircleCheck, faCircleExclamation, faTriangleExclamation, faXmark } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { Alert, Anchor, Button, Collapse, Group, List, Paper, Progress, Stack, Stepper, Text } from '@mantine/core';
import { useState } from 'react';
import { ModpackJob } from './types';

/** The steps of an install, in plain words. */
const STEPS: Record<string, { label: string; describe: (job: ModpackJob) => string }> = {
  download: { label: 'Download the modpack', describe: () => 'Getting the pack and checking it.' },
  server: {
    label: 'Prepare the server',
    describe: () =>
      'Setting up the right Minecraft version. The server shows as "installing" for a few minutes; that is normal.',
  },
  mods: {
    label: 'Install the mods',
    describe: (job) =>
      job.server_pack
        ? 'Unpacking the ready-made server files from the pack author.'
        : job.total
          ? `${Math.min(job.done, job.total)} of ${job.total} files`
          : 'Downloading the mods.',
  },
  files: { label: 'Copy the settings', describe: () => 'Configs and other files from the pack.' },
  check: { label: 'Final check', describe: () => 'Making sure every file arrived in one piece.' },
};

const minutes = (from: number, to: number) => {
  const s = Math.max(0, Math.round(to - from));
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  return m < 60 ? `${m} min` : `${Math.floor(m / 60)} h ${m % 60} min`;
};

/** A modpack install, for people who do not read logs: numbered steps, one result line, details folded away. */
export default function JobProgress({ job }: { job: ModpackJob }) {
  const [details, setDetails] = useState(false);
  const phases = job.phases?.length ? job.phases : ['download', 'mods', 'check'];
  const current = job.state === 'done' ? phases.length : Math.max(0, phases.indexOf(job.phase ?? phases[0]));
  const running = job.state === 'running';
  const failed = job.state === 'failed';
  const attention = job.failed.length + job.manual.length;
  const now = Date.now() / 1000;

  return (
    <Paper withBorder radius='md' p='md'>
      <Group justify='space-between' mb='md' wrap='nowrap'>
        <div>
          <Text fw={600}>
            {running ? 'Installing' : failed ? 'Could not install' : 'Installed'} {job.title}
          </Text>
          <Text size='xs' c='dimmed'>
            Version {job.version} ·{' '}
            {running
              ? `running for ${minutes(job.started, now)}`
              : job.finished
                ? `took ${minutes(job.started, job.finished)}, finished ${new Date(job.finished * 1000).toLocaleString()}`
                : ''}
          </Text>
        </div>
      </Group>

      <Stepper active={current} orientation='vertical' size='sm' allowNextStepsSelect={false}>
        {phases.map((p, i) => {
          const step = STEPS[p] ?? { label: p, describe: () => '' };
          const here = i === current;
          return (
            <Stepper.Step
              key={p}
              label={step.label}
              loading={running && here}
              color={failed && here ? 'red' : undefined}
              icon={failed && here ? <FontAwesomeIcon icon={faXmark} /> : undefined}
              description={
                failed && here ? (
                  'Stopped here.'
                ) : here && running ? (
                  <Stack gap={4} mt={2}>
                    <Text size='xs' c='dimmed'>
                      {step.describe(job)}
                    </Text>
                    {p === 'mods' && !job.server_pack && job.total > 0 && (
                      <Progress value={(job.done / job.total) * 100} size='sm' animated w={260} />
                    )}
                  </Stack>
                ) : undefined
              }
            />
          );
        })}
      </Stepper>

      {job.state === 'done' && attention === 0 && (
        <Alert color='green' mt='md' icon={<FontAwesomeIcon icon={faCircleCheck} />} title='Your modpack is ready'>
          Start the server to play. The first start can take a few minutes while it sets everything up.
        </Alert>
      )}
      {job.state === 'done' && attention > 0 && (
        <Alert
          color='yellow'
          mt='md'
          icon={<FontAwesomeIcon icon={faTriangleExclamation} />}
          title={`Installed, but ${attention} file${attention === 1 ? '' : 's'} need${attention === 1 ? 's' : ''} attention`}
        >
          {job.manual.length > 0 && (
            <>
              <Text size='sm'>
                These are not allowed to be downloaded automatically. Download them yourself and upload them in the
                Files tab:
              </Text>
              <List size='sm' mt={4}>
                {job.manual.map((m) => (
                  <List.Item key={m.path}>
                    {m.title ?? m.path}{' '}
                    {m.page && (
                      <Anchor href={m.page} target='_blank' rel='noreferrer' size='sm'>
                        download
                      </Anchor>
                    )}{' '}
                    → /{m.path}
                  </List.Item>
                ))}
              </List>
            </>
          )}
          {job.failed.length > 0 && (
            <Text size='sm' mt={job.manual.length ? 'xs' : 0}>
              {job.failed.length} file{job.failed.length === 1 ? '' : 's'} could not be installed. Click{' '}
              <b>Reinstall</b> below to try again; files that are already there are kept.
            </Text>
          )}
        </Alert>
      )}
      {failed && (
        <Alert color='red' mt='md' icon={<FontAwesomeIcon icon={faCircleExclamation} />} title='The install stopped'>
          <Text size='sm'>{job.step}</Text>
          <Text size='xs' c='dimmed' mt={4}>
            Fix the problem above if it names one, then click <b>Reinstall</b> below to try again.
          </Text>
        </Alert>
      )}

      <Button variant='subtle' size='compact-xs' mt='sm' onClick={() => setDetails((d) => !d)}>
        {details ? 'Hide details' : 'Show details'}
      </Button>
      <Collapse expanded={details}>
        <Stack gap={4} mt='xs'>
          <Text size='xs' c='dimmed'>
            Step: {job.step}
            {job.total > 0 && ` · ${job.done} of ${job.total}`}
            {job.installed > 0 && ` · ${job.installed} downloaded`}
            {job.written > 0 && ` · ${job.written} config files`}
            {job.removed > 0 && ` · ${job.removed} old mods removed`}
          </Text>
          {(job.switched ?? []).map((s) => (
            <Text key={s} size='xs' c='dimmed'>
              {s}
            </Text>
          ))}
          {job.warnings.map((w) => (
            <Text key={w} size='xs' c='yellow'>
              {w}
            </Text>
          ))}
          {job.failed.length > 0 && (
            <List size='xs'>
              {job.failed.slice(0, 50).map((f) => (
                <List.Item key={f}>{f}</List.Item>
              ))}
            </List>
          )}
          {job.skipped.length > 0 && (
            <Text size='xs' c='dimmed'>
              Left out {job.skipped.length} client-only: {job.skipped.join(', ')}
            </Text>
          )}
        </Stack>
      </Collapse>
    </Paper>
  );
}
