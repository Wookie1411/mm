import { axiosInstance as http } from '@/api/axios.ts';
import {
  Context,
  CrashAnalysis,
  CrashSource,
  CurrentModpack,
  ImportPreview,
  InstalledList,
  InstallResult,
  ModList,
  ModpackJob,
  ModpackPlan,
  ProjectDetail,
  ProviderId,
  SearchPage,
  Settings,
  Target,
  Version,
} from './types';
import { PackContents } from './zip';

const base = (server: string) => `/api/client/servers/${server}/mod-manager`;

export const getSettings = async (server: string): Promise<Settings> => {
  const { data } = await http.get(`${base(server)}/settings`);
  return data;
};

const settingsPromises = new Map<string, Promise<Settings>>();
const SETTINGS_TTL_MS = 15_000;

/**
 * Settings shared between the sidebar filter and the page when they load together. Kept only
 * briefly, so admin changes (eggs, a new CurseForge key) show up without reloading the browser.
 */
export const getSettingsCached = (server: string): Promise<Settings> => {
  let promise = settingsPromises.get(server);
  if (!promise) {
    promise = getSettings(server).catch((e) => {
      settingsPromises.delete(server);
      throw e;
    });
    settingsPromises.set(server, promise);
    setTimeout(() => settingsPromises.delete(server), SETTINGS_TTL_MS);
  }
  return promise;
};

export const clearSettingsCache = () => settingsPromises.clear();

export const getContext = async (server: string): Promise<Context> => {
  const { data } = await http.get(`${base(server)}/context`);
  return data;
};

export interface SearchParams extends Target {
  provider: ProviderId;
  query: string;
  sort: string;
  page: number;
  per_page: number;
  server_only: boolean;
}

export const search = async (server: string, params: SearchParams): Promise<SearchPage> => {
  const { data } = await http.get(`${base(server)}/search`, { params });
  return data.page;
};

export const getProject = async (server: string, provider: ProviderId, id: string): Promise<ProjectDetail> => {
  const { data } = await http.get(`${base(server)}/projects/${provider}/${id}`);
  return data.project;
};

export const getVersions = async (
  server: string,
  provider: ProviderId,
  id: string,
  target: Target,
): Promise<Version[]> => {
  const { data } = await http.get(`${base(server)}/projects/${provider}/${id}/versions`, { params: target });
  return data.versions;
};

export const getInstalled = async (server: string, target: Target, fresh = false): Promise<InstalledList> => {
  const { data } = await http.get(`${base(server)}/installed`, {
    params: fresh ? { ...target, fresh: 1 } : target,
  });
  return data;
};

export interface InstallPayload {
  provider: ProviderId;
  project_id: string;
  version_id?: string | null;
  replace?: string | null;
  dependencies: boolean;
}

export const install = async (server: string, target: Target, payload: InstallPayload): Promise<InstallResult> => {
  const { data } = await http.post(`${base(server)}/install`, { ...target, ...payload });
  return data;
};

export const toggle = async (server: string, target: Target, file: string, enabled: boolean): Promise<string> => {
  const { data } = await http.post(`${base(server)}/toggle`, { ...target, file, enabled });
  return data.file;
};

export const remove = async (server: string, target: Target, files: string[]): Promise<void> => {
  await http.post(`${base(server)}/remove`, { ...target, files });
};

export const exportList = async (server: string, target: Target): Promise<ModList> => {
  const { data } = await http.get(`${base(server)}/export`, { params: target });
  return data;
};

export const previewImport = async (
  server: string,
  target: Target,
  list: ModList,
  exact: boolean,
): Promise<ImportPreview> => {
  const { data } = await http.post(`${base(server)}/import/preview`, { ...target, list, exact });
  return data;
};

/** Turn an unzipped launcher pack (see zip.ts) into a mod list. */
export const importPack = async (server: string, pack: PackContents): Promise<ModList> => {
  const { data } = await http.post(`${base(server)}/import/pack`, pack);
  return data;
};

export interface ModpackRequest {
  provider: ProviderId;
  project_id: string;
  version_id: string;
  clean?: boolean;
  switch_version?: boolean;
  wipe?: boolean;
  server_pack?: boolean;
  /** Write eula.txt with eula=true after installing. */
  accept_eula?: boolean;
}

export const modpackPlan = async (server: string, req: ModpackRequest): Promise<ModpackPlan> => {
  const { data } = await http.post(`${base(server)}/modpack/plan`, req, { timeout: 180_000 });
  return data;
};

export const modpackInstall = async (server: string, req: ModpackRequest): Promise<void> => {
  await http.post(`${base(server)}/modpack/install`, req, { timeout: 180_000 });
};

export const modpackJob = async (server: string): Promise<ModpackJob | null> => {
  const { data } = await http.get(`${base(server)}/modpack/job`);
  return data.job;
};

export const modpackCurrent = async (server: string, target: Target): Promise<CurrentModpack | null> => {
  const { data } = await http.get(`${base(server)}/modpack/current`, { params: target });
  return data.modpack;
};

/** Forget the server's modpack (files stay), e.g. after the server was wiped. */
export const modpackForget = async (server: string): Promise<void> => {
  await http.delete(`${base(server)}/modpack/current`);
};

export const crashSources = async (server: string): Promise<CrashSource[]> => {
  const { data } = await http.get(`${base(server)}/crash/sources`);
  return data.sources;
};

/** Analyse a log from the server (`source`) or a pasted one (`text`). */
export const analyzeCrash = async (server: string, req: { source?: string; text?: string }): Promise<CrashAnalysis> => {
  const { data } = await http.post(`${base(server)}/crash/analyze`, req, { timeout: 180_000 });
  return data;
};

// ------------------------------------------------------------------ admin

export interface AdminSettings {
  access: 'admins' | 'everyone' | 'users';
  users: string[];
  eggs: string[];
  modrinth_enabled: boolean;
  curseforge_enabled: boolean;
  switch_eggs: boolean;
  curseforge_key_set: boolean;
}

export interface AdminEgg {
  uuid: string;
  name: string;
  nest: string;
}

const adminBase = '/api/admin/extensions/dev.emrekmek.modmanager';

export interface AdminUser {
  uuid: string;
  username: string;
  email: string;
  admin: boolean;
}

export const getAdminUsers = async (): Promise<AdminUser[]> => {
  const { data } = await http.get(`${adminBase}/users`);
  return data.users;
};

export const getAdminSettings = async (): Promise<AdminSettings> => {
  const { data } = await http.get(`${adminBase}/settings`);
  return data.settings;
};

export const getAdminEggs = async (): Promise<AdminEgg[]> => {
  const { data } = await http.get(`${adminBase}/eggs`);
  return data.eggs;
};

export interface KeyTest {
  ok: boolean;
  status: number | null;
  source: 'typed' | 'stored';
  message: string;
  shape: { length: number; looks_like_a_key: boolean; has_whitespace: boolean };
}

/** Try the typed key (or the stored one when empty) against the CurseForge API. */
export const testCurseforgeKey = async (key: string | null): Promise<KeyTest> => {
  const { data } = await http.post(`${adminBase}/test-curseforge`, { key: key || null });
  return data;
};

export const updateAdminSettings = async (
  settings: Partial<Omit<AdminSettings, 'curseforge_key_set'>> & { curseforge_api_key?: string },
): Promise<AdminSettings> => {
  const { data } = await http.put(`${adminBase}/settings`, settings);
  return data.settings;
};

export interface UpdateCheck {
  current: string;
  repository: string;
  latest: { version: string; tag: string; notes: string; published: string | null; url: string } | null;
  update_available: boolean;
  can_install: boolean;
  panel_version: string;
}

export const checkForUpdate = async (): Promise<UpdateCheck> => {
  const { data } = await http.get(`${adminBase}/update`);
  return data;
};

export const installUpdate = async (): Promise<{ installed: string; build_id: number | null; message: string }> => {
  const { data } = await http.post(`${adminBase}/update`, {}, { timeout: 300_000 });
  return data;
};

export interface EggStatus {
  installed: {
    uuid: string;
    name: string;
    nest: string;
    nest_name: string;
    revision: number;
    outdated: boolean;
  } | null;
  revision: number;
  nests: { uuid: string; name: string; minecraft_eggs: number }[];
}

export const getAnyLoaderEgg = async (): Promise<EggStatus> => {
  const { data } = await http.get(`${adminBase}/egg`);
  return data;
};

export const addAnyLoaderEgg = async (nest: string): Promise<void> => {
  await http.post(`${adminBase}/egg`, { nest });
};

export const downloadAnyLoaderEgg = async (): Promise<Blob> => {
  const { data } = await http.get(`${adminBase}/egg/download`, { responseType: 'blob' });
  return data;
};
