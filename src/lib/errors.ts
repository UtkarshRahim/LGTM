import type { AppError } from "../ipc";

export type ErrorAction = { label: string; kind: "switch_account" } | { label: string; kind: "open_url"; url: string };

export interface Described {
  title: string;
  detail?: string;
  actions: ErrorAction[];
}

export function describe(e: AppError): Described {
  if (e.kind !== "github") return { title: e.message, actions: [] };
  const g = e.error;
  switch (g.kind) {
    case "not_visible":
      return {
        title: `${g.what} isn't visible to ${g.login}`,
        detail: "GitHub reports private repos you can't access the same way as ones that don't exist.",
        actions: [{ label: "Switch account", kind: "switch_account" }],
      };
    case "sso_required":
      return {
        title: `${g.org} requires SAML single sign-on`,
        detail: "Authorize your token for this organization, then try again.",
        actions: [{ label: "Authorize SSO", kind: "open_url", url: g.url }],
      };
    case "no_account":
      return {
        title: `No GitHub account for ${g.host}`,
        detail: "Run `gh auth login` or set GH_TOKEN, then refresh accounts.",
        actions: [{ label: "Refresh accounts", kind: "switch_account" }],
      };
    case "unauthorized":
      return {
        title: `GitHub rejected the token for ${g.host}`,
        detail: "Run `gh auth refresh` or pick another account.",
        actions: [{ label: "Switch account", kind: "switch_account" }],
      };
    case "rate_limited":
      return {
        title: "GitHub rate limit reached",
        detail: g.reset_at ? `Resets at ${new Date(g.reset_at * 1000).toLocaleTimeString()}.` : undefined,
        actions: [],
      };
    case "api":
    case "network":
      return { title: g.message, actions: [] };
  }
}

export function errorText(e: AppError): string {
  return describe(e).title;
}
