// Values coming from Rust use the `_Serialize` shapes (every field present);
// re-export them under plain names so components never see the suffixes.
export { commands, events } from "./bindings";
export type {
  Account,
  Accounts_Serialize as Accounts,
  AppError_Serialize as AppError,
  AppInfo_Serialize as AppInfo,
  CheckoutMode,
  CheckoutOutcome_Serialize as CheckoutOutcome,
  DiffLine_Serialize as DiffLine,
  FileDiff_Serialize as FileDiff,
  GhError_Serialize as GhError,
  Hunk_Serialize as Hunk,
  PrCount_Serialize as PrCount,
  PrDetail_Serialize as PrDetail,
  PrHit_Serialize as PrHit,
  PrRef,
  PrView_Serialize as PrView,
  Project_Serialize as Project,
  ProjectStatus_Serialize as ProjectStatus,
  Settings_Serialize as Settings,
  ShellEnvStatus_Serialize as ShellEnvStatus,
} from "./bindings";

import type { PrRef } from "./bindings";

export const prKey = (pr: PrRef) => `${pr.host}/${pr.owner}/${pr.repo}#${pr.number}`;
export const prLabel = (pr: PrRef) => `${pr.owner}/${pr.repo}#${pr.number}`;
