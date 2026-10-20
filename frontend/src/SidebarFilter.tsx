import { useEffect, useState } from 'react';
import { useServerStore } from '@/stores/server.ts';
import { getSettingsCached } from './api';

/**
 * Mounted on every server page: hides this extension's sidebar entry unless the API says the
 * page applies to this server (egg filter, admin-only, permissions). Hidden until that answer
 * arrives, so the link never flashes up on servers it does not belong to.
 */
export default function SidebarFilter() {
  const uuid = useServerStore((state) => state.server.uuid);
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    let alive = true;
    setVisible(false);
    if (uuid) {
      getSettingsCached(uuid)
        .then((s) => alive && setVisible(s.egg_allowed && !s.restricted && s.can_view))
        .catch(() => alive && setVisible(false));
    }
    return () => {
      alive = false;
    };
  }, [uuid]);

  return visible ? null : <style>{'a[href$="/mod-manager"] { display: none !important; }'}</style>;
}
