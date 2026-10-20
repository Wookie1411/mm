import { MultiSelect, SegmentedControl, Stack, Text } from '@mantine/core';
import { useEffect, useState } from 'react';
import { AdminUser, getAdminUsers } from './api';

export type Access = 'admins' | 'everyone' | 'users';

const DESCRIPTIONS: Record<Access, string> = {
  admins: 'Only panel administrators can open the page.',
  everyone:
    'Anyone who can open a server can use it there, within their server permissions (subusers still need the matching permission).',
  users:
    'Panel administrators plus the users picked below, within their server permissions (subusers still need the matching permission).',
};

/** Who may use an extension's page: administrators, everyone, or picked users. */
export default function AccessSetting({
  access,
  users,
  onChange,
}: {
  access: Access;
  users: string[];
  onChange: (access: Access, users: string[]) => void;
}) {
  const [all, setAll] = useState<AdminUser[]>([]);

  useEffect(() => {
    getAdminUsers()
      .then(setAll)
      .catch(() => setAll([]));
  }, []);

  // administrators always have access, so they are not offered
  const data = all.filter((u) => !u.admin).map((u) => ({ value: u.uuid, label: `${u.username} (${u.email})` }));

  return (
    <Stack gap={6}>
      <Text size='sm' fw={500}>
        Who can use it
      </Text>
      <SegmentedControl
        value={access}
        onChange={(v) => onChange(v as Access, users)}
        data={[
          { value: 'admins', label: 'Only panel administrators' },
          { value: 'everyone', label: 'Everyone' },
          { value: 'users', label: 'Only selected users' },
        ]}
      />
      <Text size='xs' c='dimmed'>
        {DESCRIPTIONS[access]}
      </Text>
      {access === 'users' && (
        <MultiSelect
          label='Users'
          placeholder={data.length ? 'Pick users' : 'No users other than administrators'}
          data={data}
          value={users}
          onChange={(v) => onChange(access, v)}
          searchable
          clearable
        />
      )}
    </Stack>
  );
}
