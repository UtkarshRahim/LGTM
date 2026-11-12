// Values coming from Rust use the `_Serialize` shapes (every field present);
// re-export them under plain names so components never see the suffixes.
export { commands, events, HIGHLIGHT_CLASSES } from "./bindings";
export type {
  Account,
  Accounts_Serialize as Accounts,
  AppError_Serialize as AppError,
  AppInfo_Serialize as AppInfo,
  CheckoutMode,
  CheckoutOutcome_Serialize as CheckoutOutcome,
  DiffLayout,
  DiffLine_Serialize as DiffLine,
  FileHunks_Serialize as FileHunks,
  FileMeta_Serialize as FileMeta,
  GhError_Serialize as GhError,
  Hunk_Serialize as Hunk,
  NeedsYou_Serialize as NeedsYou,
  PrCount_Serialize as PrCount,
  PrDetail_Serialize as PrDetail,
  PrHit_Serialize as PrHit,
  PrRef,
  PrView_Serialize as PrView,
  Project_Serialize as Project,
  ProjectStatus_Serialize as ProjectStatus,
  ReviewComment_Serialize as ReviewComment,
  ReviewThread_Serialize as ReviewThread,
  Settings_Serialize as Settings,
  ShellEnvStatus_Serialize as ShellEnvStatus,
} from "./bindings";

import type { PrRef } from "./bindings";

export const prKey = (pr: PrRef) => `${pr.host}/${pr.owner}/${pr.repo}#${pr.number}`;
export const prLabel = (pr: PrRef) => `${pr.owner}/${pr.repo}#${pr.number}`;
