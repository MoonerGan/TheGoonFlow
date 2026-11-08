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
  launchPath: string | null;
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

export type VideoImportReport = {
  imported: number;
  skipped: number;
  withThumb: number;
  stopped: boolean;
  titles: string[];
};

export type EnrichReport = {
  sources: number;
  enriched: number;
  stopped: boolean;
  notes: string[];
  failures: { url: string; reason: string }[];
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
  finishedAt: number;
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
export const deleteItems = (ids: string[]) => invoke<number>("delete_items", { ids });
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
export const importVideoFiles = (paths: string[]) =>
  invoke<VideoImportReport>("import_video_files", { paths });
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
export const pairingSuggestions = (kind?: "game" | "media") =>
  invoke<PairSuggestion[]>("pairing_suggestions", { kind: kind ?? null });
export const mergeItems = (keepId: string, dropId: string) =>
  invoke<void>("merge_items", { keepId, dropId });

// ---------- per-item source + re-enrichment (M2.6) ----------
export const reEnrichItem = (itemId: string) =>
  invoke<EnrichReport>("re_enrich_item", { itemId });
export const addSource = (itemId: string, url: string) =>
  invoke<string>("add_source", { itemId, url });
export const removeSource = (itemId: string, sourceId: number) =>
  invoke<void>("remove_source", { itemId, sourceId });

// ---------- launch (#18) ----------
export type LaunchPlan = {
  ok: boolean;
  exe: string | null;
  cwd: string | null;
  engine: string | null;
  note: string | null;
  pid: number | null;
};
export const previewLaunch = (itemId: string) =>
  invoke<LaunchPlan>("preview_launch", { itemId });
export const launchItem = (itemId: string) =>
  invoke<LaunchPlan>("launch_item", { itemId });
export const setLaunchPath = (itemId: string, path: string | null) =>
  invoke<void>("set_launch_path", { itemId, path });

// ---------- online match suggestions (#14) ----------
export type MatchSuggestion = {
  itemId: string;
  itemTitle: string;
  site: string;
  url: string;
  title: string;
  confidence: number;
  foundAt: string;
};
export type MatchReport = {
  scanned: number;
  withSuggestions: number;
  suggestions: number;
  failures: string[];
  stopped: boolean;
};
export const findMatches = () => invoke<MatchReport>("find_matches");
export const listMatchSuggestions = (limit = 200) =>
  invoke<MatchSuggestion[]>("list_match_suggestions", { limit });
export const dismissSuggestion = (itemId: string, site: string, url: string) =>
  invoke<void>("dismiss_suggestion", { itemId, site, url });
export const clearItemSuggestions = (itemId: string) =>
  invoke<void>("clear_item_suggestions", { itemId });

// ---------- vault video storage (#16) ----------
export type VideoInfo = {
  hasSource: boolean;
  sourceSize: number | null;
  sourceKind: string | null;
  stored: boolean;
  storedSize: number | null;
};
export type VideoStoreReport = {
  storedBytes: number;
  thumbAdded: boolean;
  note: string | null;
};
export const videoInfo = (itemId: string) => invoke<VideoInfo>("video_info", { itemId });
export const storeVideo = (itemId: string) => invoke<VideoStoreReport>("store_video", { itemId });
export const removeStoredVideo = (itemId: string) =>
  invoke<void>("remove_stored_video", { itemId });

// ---------- proxy / Tor (#24) ----------
export type ProxySettings = { proxyUrl: string | null };
export const getProxy = () => invoke<ProxySettings>("get_proxy");
export const setProxy = (url: string | null) =>
  invoke<ProxySettings>("set_proxy", { url });

// ---------- activity (jobs) ----------
export const listJobs = () => invoke<JobInfo[]>("list_jobs");
export const cancelJob = (id: number) => invoke<void>("cancel_job", { id });
