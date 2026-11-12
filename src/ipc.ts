// Values coming from Rust use the `_Serialize` shapes (every field present);
// re-export them under plain names so components never see the suffixes.
export { commands } from "./bindings";
export type {
  AppError,
  AppInfo_Serialize as AppInfo,
  Settings_Serialize as Settings,
  ShellEnvStatus_Serialize as ShellEnvStatus,
} from "./bindings";
