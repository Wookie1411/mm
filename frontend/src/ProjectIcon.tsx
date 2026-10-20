import { faCube } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { useState } from 'react';

/** Project icon with a cube placeholder when there is none or it fails to load. */
export default function ProjectIcon({ url, title, size = 56 }: { url: string | null; title: string; size?: number }) {
  const [failed, setFailed] = useState(false);
  const box = {
    width: size,
    height: size,
    flexShrink: 0,
    borderRadius: 'var(--mantine-radius-md)',
  };

  if (!url || failed) {
    return (
      <div
        style={{
          ...box,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          background: 'var(--mantine-color-default-hover)',
          color: 'var(--mantine-color-dimmed)',
          fontSize: size / 2.4,
        }}
      >
        <FontAwesomeIcon icon={faCube} />
      </div>
    );
  }

  return (
    <img
      src={url}
      alt={title}
      loading='lazy'
      referrerPolicy='no-referrer'
      onError={() => setFailed(true)}
      style={{ ...box, objectFit: 'cover' }}
    />
  );
}
