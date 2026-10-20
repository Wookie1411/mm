import { ProviderId, Target } from './types';

export const LOADER_LABELS: Record<string, string> = {
  fabric: 'Fabric',
  quilt: 'Quilt',
  forge: 'Forge',
  neoforge: 'NeoForge',
  paper: 'Paper',
  purpur: 'Purpur',
  folia: 'Folia',
  spigot: 'Spigot',
  bukkit: 'Bukkit',
  sponge: 'Sponge',
  velocity: 'Velocity',
  bungeecord: 'BungeeCord',
  waterfall: 'Waterfall',
};

export const PLUGIN_LOADERS = [
  'paper',
  'purpur',
  'folia',
  'spigot',
  'bukkit',
  'sponge',
  'velocity',
  'bungeecord',
  'waterfall',
];

export const isPluginLoader = (loader: string | null | undefined) => !!loader && PLUGIN_LOADERS.includes(loader);

export const loaderLabel = (loader: string | null | undefined) =>
  loader ? (LOADER_LABELS[loader] ?? loader) : 'Unknown';

export const PROVIDER_LABELS: Record<ProviderId, string> = {
  modrinth: 'Modrinth',
  curseforge: 'CurseForge',
};

export const SORTS = [
  { value: 'relevance', label: 'Relevance' },
  { value: 'downloads', label: 'Downloads' },
  { value: 'follows', label: 'Popularity' },
  { value: 'newest', label: 'Newest' },
  { value: 'updated', label: 'Recently updated' },
];

export const formatNumber = (n: number | null | undefined) => (n == null ? '—' : Math.round(n).toLocaleString());

export const formatBytes = (n: number) => {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  return `${(n / 1024 / 1024).toFixed(1)} MiB`;
};

export const formatDate = (iso: string | null | undefined) => {
  if (!iso) return '—';
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? '—' : d.toLocaleDateString();
};

export const versionTypeColor = (t: string) => (t === 'release' ? 'green' : t === 'beta' ? 'yellow' : 'red');

/** "1.20, 1.20.1, 1.20.2 … 1.21" -> first and last few, so long lists stay on one line. */
export const shortVersions = (versions: string[], max = 4) =>
  versions.length <= max ? versions.join(', ') : `${versions.slice(0, max).join(', ')} +${versions.length - max}`;

export const projectKey = (provider: string, id: string) => `${provider}:${id}`;

/** What to do so the server picks up a change. */
export const applyHint = (target: Target | null) =>
  target?.kind === 'datapack' ? 'Run /reload or restart the server to apply.' : 'Restart the server to apply.';
