export type ProviderId = 'modrinth' | 'curseforge';

export interface Settings {
  access: string;
  restricted: boolean;
  egg_allowed: boolean;
  can_view: boolean;
  can_manage: boolean;
  providers: ProviderId[];
  curseforge_disabled_reason: string | null;
}

export interface Detected {
  loader: string | null;
  game_version: string | null;
  directory: string | null;
  datapack_directory: string;
  notes: string[];
}

export interface Context {
  detected: Detected;
  loaders: string[];
  game_versions: string[];
}

export interface Project {
  provider: ProviderId;
  id: string;
  slug: string;
  title: string;
  description: string;
  author: string | null;
  downloads: number;
  follows: number;
  icon_url: string | null;
  url: string;
  categories: string[];
  updated: string | null;
  /** Modrinth only: required / optional / unsupported / unknown. */
  server_side: string | null;
}

export interface ProjectDetail extends Project {
  body: string | null;
  links: { label: string; url: string }[];
  gallery: string[];
  loaders: string[];
  game_versions: string[];
  distribution_blocked: boolean;
}

export interface SearchPage {
  hits: Project[];
  total: number;
  offset: number;
  limit: number;
}

export interface FileInfo {
  name: string;
  size: number;
  url: string | null;
  sha1: string | null;
}

export interface Dependency {
  project_id: string | null;
  version_id: string | null;
  kind: 'required' | 'optional' | 'incompatible' | 'embedded' | string;
}

export interface Version {
  provider: ProviderId;
  id: string;
  project_id: string;
  name: string;
  version_number: string;
  version_type: 'release' | 'beta' | 'alpha' | string;
  game_versions: string[];
  loaders: string[];
  published: string | null;
  downloads: number;
  file: FileInfo | null;
  dependencies: Dependency[];
}

export interface Installed {
  file: string;
  size: number;
  modified: number;
  enabled: boolean;
  sha1: string | null;
  provider: ProviderId | null;
  project: Project | null;
  version: Version | null;
  update: Version | null;
}

export interface InstalledList {
  directory: string | null;
  loader: string | null;
  game_version: string | null;
  mods: Installed[];
  warnings: string[];
  generated_at: number;
}

export interface InstallResult {
  installed: { title: string; file: string; version: string; replaced: string | null; required_by: string | null }[];
  warnings: string[];
}

export type ContentKind = 'mods' | 'plugins' | 'datapack' | 'modpack';

/** The loader / Minecraft version the page is filtered to ('any' = no filter). */
export interface Target {
  kind: ContentKind;
  loader: string;
  game_version: string;
}

export interface ModListEntry {
  provider: ProviderId;
  project_id: string;
  slug: string;
  title: string;
  version_id: string | null;
  version_number: string | null;
  file: string | null;
  enabled: boolean;
}

export interface ModList {
  format: string;
  format_version: number;
  exported_at: string | null;
  server: string | null;
  loader: string | null;
  game_version: string | null;
  mods: ModListEntry[];
  unrecognised: string[];
  /** Client-only mods left out of an imported launcher pack. */
  skipped?: string[];
}

export interface ImportRow {
  entry: ModListEntry;
  status: 'ready' | 'installed' | 'incompatible' | 'blocked' | 'error';
  message: string | null;
  version: Version | null;
  installed_file: string | null;
}

export interface ImportPreview {
  rows: ImportRow[];
  unrecognised: string[];
  skipped: string[];
  loader: string | null;
  game_version: string | null;
  directory: string | null;
  source: { server: string | null; loader: string | null; game_version: string | null };
}

export interface ModpackPlan {
  provider: ProviderId;
  version: Version;
  requires: { minecraft: string | null; loader: string | null; loader_version: string | null };
  server: { loader: string | null; game_version: string | null };
  warnings: string[];
  /** CurseForge: the server pack linked to this file; url null when downloads are blocked. */
  server_pack: { name: string; size: number; url: string | null; sha1: string | null } | null;
  switch_version: {
    supported: boolean;
    reason: string | null;
    how: { route: 'jar' } | { route: 'egg'; egg: string; moves: boolean } | null;
  };
  downloads: { path: string; title: string | null; size: number }[];
  download_bytes: number;
  manual: { path: string; title: string | null; page: string | null }[];
  skipped: string[];
  rejected: string[];
  overrides: number;
  override_bytes: number;
  existing_mods: number;
}

export interface ModpackJob {
  state: 'running' | 'done' | 'failed';
  title: string;
  version: string;
  step: string;
  total: number;
  done: number;
  installed: number;
  written: number;
  removed: number;
  failed: string[];
  manual: { path: string; title: string | null; page: string | null }[];
  skipped: string[];
  warnings: string[];
  switched?: string[];
  started: number;
  finished: number | null;
}

export interface CurrentModpack {
  provider: ProviderId;
  project_id: string;
  version_id: string;
  title: string;
  version_number: string;
  installed: string;
  project: Project | null;
  update: Version | null;
  installed_version: Version | null;
}

export interface CrashSource {
  path: string;
  label: string;
  modified: number;
  size: number;
}

export type CrashAction =
  | { type: 'disable'; kind: ContentKind; file: string; label: string }
  | { type: 'versions'; kind: ContentKind; provider: ProviderId; project_id: string; label: string }
  | { type: 'install'; kind: ContentKind; provider: ProviderId; project_id: string; label: string }
  | { type: 'search'; kind: ContentKind; query: string; label: string };

export interface CrashFinding {
  severity: 'error' | 'warning' | 'info';
  title: string;
  detail: string;
  evidence: string[];
  actions: CrashAction[];
  /** Other ways to fix it, folded away. */
  alternatives: CrashAction[];
}

export interface CrashAnalysis {
  source: string;
  lines: number;
  crashed: boolean;
  summary: string;
  headline: string[];
  findings: CrashFinding[];
}
