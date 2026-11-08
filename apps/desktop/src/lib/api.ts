import { invoke } from "@tauri-apps/api/core";

export type Volume = {
  letter: string;
  label: string | null;
  root: string;
  removable: boolean;
};

export type CompanionStatus = {
  port: number;
  token: string;
};

export type AppStatus = {
  hasVault: boolean;
  vaultResolved: boolean;
  unlocked: boolean;
  vaultRoot: string | null;
  companion: CompanionStatus | null;
};

export type StatusCount = {
  status: string;
  count: number;
};

export const getStatus = () => invoke<AppStatus>("app_status");
export const getDefaultVaultDir = () => invoke<string>("default_vault_dir");
export const listVolumes = () => invoke<Volume[]>("list_volumes");
export const createVault = (dir: string, passphrase: string) =>
  invoke<void>("create_vault", { dir, passphrase });
export const attachVault = (dir: string) => invoke<void>("attach_vault", { dir });
export const unlockVault = (passphrase: string) =>
  invoke<void>("unlock", { passphrase });
export const lockVault = () => invoke<void>("lock");
export const nukeVault = (confirm: string, dir?: string) =>
  invoke<void>("nuke_vault", { confirm, dir: dir ?? null });
export const resetPointer = () => invoke<void>("reset_pointer");
export const getStats = (kind?: string) =>
  invoke<StatusCount[]>("vault_stats", { kind: kind ?? null });
export const getKindStats = () => invoke<Array<[string, number]>>("kind_stats");

export type ItemSummary = {
  id: string;
  kind: string;
  title: string;
  status: string;
  version: string | null;
  hasThumb: boolean;
  updatedAt: string;
};

export type SourceRow = {
  id: number;
  site: string;
  url: string;
  sourceId: string | null;
  fetchedAt: string | null;
  dead: boolean;
};

export type TagOnItem = {
  id: number;
  name: string;
  typeKey: string;
};

export type ItemDetail = {
  id: string;
  kind: string;
  title: string;
  originalTitle: string | null;
  circle: string | null;
  status: string;
  version: string | null;
  notes: string;
  createdAt: string;
  updatedAt: string;
  hasThumb: boolean;
  sources: SourceRow[];
  tags: TagOnItem[];
};

export type TagCount = {
  id: number;
  name: string;
  typeKey: string;
  count: number;
};

export type ThumbPayload = { id: string; mime: string; data: string };

export type GameImportReport = {
  imported: number;
  skipped: number;
  withThumb: number;
  stopped: boolean;
  titles: string[];
  failures: string[];
};

export type ImageImportReport = {
  itemId: string;
  title: string;
  imagesStored: number;
  failed: number;
  stopped: boolean;
};

export type JobInfo = {
  id: number;
  kind: string;
  label: string;
  done: number;
  total: number;
  current: string;
  note: string;
  finished: boolean;
  cancelled: boolean;
  startedAt: number;
};

export type FailureDetail = { url: string; reason: string };

export type BookmarkReport = {
  parsed: number;
  imported: number;
  duplicates: number;
  unrecognized: number;
  disabled: number;
  thumbnailed: number;
  tagsAdded: number;
  fetchFailures: number;
  stopped: boolean;
  bySite: Record<string, number>;
  failures: FailureDetail[];
  unrecognizedSample: string[];
};

export type ClipReport = {
  itemId: string;
  title: string;
  site: string;
  created: boolean;
  storedImage: boolean;
  thumb: boolean;
  tags: number;
  note: string | null;
};

export type SiteInfo = {
  site: string;
  kind: string;
  mirror: boolean;
  defaultRateMs: number;
};

export type SiteSetting = {
  site: string;
  enabled: boolean;
  cookieSet: boolean;
  rateMs: number;
};

export type UpdateRow = {
  itemId: string;
  title: string;
  site: string;
  oldVersion: string | null;
  newVersion: string | null;
  seenAt: string;
};

export type UpdateReport = {
  checked: number;
  updated: number;
  failures: number;
  stopped: boolean;
  updatedTitles: string[];
};

export type SiteCount = {
  site: string;
  count: number;
};

export type PairSuggestion = {
  aId: string;
  aTitle: string;
  aSites: string;
  bId: string;
  bTitle: string;
  bSites: string;
};

export const listItems = (
  limit = 300,
  offset = 0,
  search?: string,
  kind?: string,
  tag?: string,
  site?: string,
) =>
  invoke<ItemSummary[]>("list_items", {
    limit,
    offset,
    search: search ?? null,
    kind: kind ?? null,
    tag: tag ?? null,
    site: site ?? null,
  });
export const siteStats = (kind?: string) =>
  invoke<SiteCount[]>("site_stats", { kind: kind ?? null });
export const getThumbs = (ids: string[]) =>
  invoke<ThumbPayload[]>("get_thumbs", { ids });
export const updateItem = (
  id: string,
  title: string,
  circle: string | null,
  status: string,
  version: string | null,
) => invoke<void>("update_item", { id, title, circle, status, version });
export const deleteItem = (id: string) => invoke<void>("delete_item", { id });
export const setComplete = (id: string, complete: boolean) =>
  invoke<void>("set_complete", { id, complete });
export const importGameFolder = (dir: string) =>
  invoke<GameImportReport>("import_game_folder", { dir });
export const importGameExe = (exe: string) =>
  invoke<GameImportReport>("import_game_exe", { exe });
export const importGameZip = (zipPath: string) =>
  invoke<GameImportReport>("import_game_zip", { zipPath });
export const importImageFiles = (paths: string[]) =>
  invoke<ImageImportReport>("import_image_files", { paths });
export const importBookmarks = (path: string, fetchOnline: boolean) =>
  invoke<BookmarkReport>("import_bookmarks", { path, fetchOnline });

// ---------- M1: clip + tags ----------
export const clipUrl = (url: string) => invoke<ClipReport>("clip_url", { url });
export const getItem = (id: string) => invoke<ItemDetail | null>("get_item", { id });
export const topTags = (limit = 40, kind?: "game" | "media") =>
  invoke<TagCount[]>("top_tags", { limit, kind: kind ?? null });
export const addItemTag = (id: string, name: string, kind?: "content" | "site" | "status") =>
  invoke<void>("add_item_tag", { id, name, kind: kind ?? null });
export const removeItemTag = (id: string, tagId: number) =>
  invoke<void>("remove_item_tag", { id, tagId });

// ---------- M2: sources, updates, pairing ----------
export const knownSites = () => invoke<SiteInfo[]>("known_sites");
export const getSiteSettings = () => invoke<SiteSetting[]>("get_site_settings");
export const setSiteSetting = (
  site: string,
  enabled?: boolean,
  cookie?: string,
  rateMs?: number,
) =>
  invoke<void>("set_site_setting", {
    site,
    enabled: enabled ?? null,
    cookie: cookie ?? null,
    rateMs: rateMs ?? null,
  });
export const checkUpdates = () => invoke<UpdateReport>("check_updates");
export const listUpdates = (limit = 100) => invoke<UpdateRow[]>("list_updates", { limit });
export const pairingSuggestions = () =>
  invoke<PairSuggestion[]>("pairing_suggestions");
export const mergeItems = (keepId: string, dropId: string) =>
  invoke<void>("merge_items", { keepId, dropId });

// ---------- activity (jobs) ----------
export const listJobs = () => invoke<JobInfo[]>("list_jobs");
export const cancelJob = (id: number) => invoke<void>("cancel_job", { id });
