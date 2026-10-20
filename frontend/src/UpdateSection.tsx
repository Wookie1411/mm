import { faArrowUp, faRotate } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { Alert, Anchor, Button, Divider, Group, Modal, ScrollArea, Stack, Text } from '@mantine/core';
import { useState } from 'react';
import { httpErrorToHuman } from '@/api/axios.ts';
import { useToast } from '@/providers/ToastProvider.tsx';
import { checkForUpdate, installUpdate, UpdateCheck } from './api';

/** Admin settings: check GitHub for a newer Mod Manager and install it with one click. */
export default function UpdateSection() {
  const { addToast } = useToast();
  const [check, setCheck] = useState<UpdateCheck | null>(null);
  const [checking, setChecking] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [done, setDone] = useState<string | null>(null);

  const runCheck = async () => {
    setChecking(true);
    setDone(null);
    try {
      setCheck(await checkForUpdate());
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
    } finally {
      setChecking(false);
    }
  };

  const runInstall = async () => {
    setInstalling(true);
    try {
      const r = await installUpdate();
      setDone(r.message);
      setConfirm(false);
    } catch (e) {
      addToast(httpErrorToHuman(e), 'error');
    } finally {
      setInstalling(false);
    }
  };

  const latest = check?.latest;

  return (
    <Stack gap='xs'>
      <Divider label='Updates' labelPosition='left' />
      <Group gap='sm'>
        <Button
          variant='default'
          size='xs'
          leftSection={<FontAwesomeIcon icon={faRotate} spin={checking} />}
          loading={checking}
          onClick={runCheck}
        >
          Check for updates
        </Button>
        {check && (
          <Text size='sm' c='dimmed'>
            Installed: {check.current}
            {latest ? ` · newest: ${latest.version}` : ' · no release published yet'}
          </Text>
        )}
      </Group>

      {done && <Alert color='green'>{done}</Alert>}

      {check && !done && latest && check.update_available && (
        <Alert color='blue' title={`Mod Manager ${latest.version} is available`}>
          {latest.notes && (
            <ScrollArea.Autosize mah={160}>
              <Text size='sm' style={{ whiteSpace: 'pre-wrap' }}>
                {latest.notes}
              </Text>
            </ScrollArea.Autosize>
          )}
          <Group mt='sm' gap='sm'>
            {check.can_install ? (
              <Button size='xs' leftSection={<FontAwesomeIcon icon={faArrowUp} />} onClick={() => setConfirm(true)}>
                Update to {latest.version}
              </Button>
            ) : (
              <Text size='xs' c='dimmed'>
                This panel cannot install extensions (not the heavy image, or extension management is turned off).
              </Text>
            )}
            {latest.url && (
              <Anchor href={latest.url} target='_blank' rel='noreferrer' size='xs'>
                Release page
              </Anchor>
            )}
          </Group>
        </Alert>
      )}
      {check && !done && latest && !check.update_available && (
        <Text size='sm' c='green'>
          You have the newest version.
        </Text>
      )}

      <Modal opened={confirm} onClose={() => setConfirm(false)} title='Update Mod Manager' centered>
        <Text size='sm'>
          Download Mod Manager {latest?.version} from GitHub and install it? The panel then rebuilds itself in the
          background (a few minutes) and switches to the new version on its own. It keeps working with the current
          version until then.
        </Text>
        <Group justify='flex-end' mt='md'>
          <Button variant='default' onClick={() => setConfirm(false)}>
            Cancel
          </Button>
          <Button loading={installing} onClick={runInstall}>
            Update
          </Button>
        </Group>
      </Modal>
    </Stack>
  );
}
