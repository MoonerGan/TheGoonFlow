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

export type ThumbPayload = { id: string; mime: string; data: string };

export type GameImportReport = {
  imported: number;
  skipped: number;
  withThumb: number;
  titles: string[];
  failures: string[];
};

export type ImageImportReport = {
  itemId: string;
  title: string;
  imagesStored: number;
  failed: number;
};

export type FailureDetail = { url: string; reason: string };

export type BookmarkReport = {
  parsed: number;
  imported: number;
  duplicates: number;
  unrecognized: number;
  thumbnailed: number;
  tagsAdded: number;
  fetchFailures: number;
  bySite: Record<string, number>;
  failures: FailureDetail[];
  unrecognizedSample: string[];
};

export const listItems = (limit = 300, offset = 0, search?: string, kind?: string) =>
  invoke<ItemSummary[]>("list_items", {
    limit,
    offset,
    search: search ?? null,
    kind: kind ?? null,
  });
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
