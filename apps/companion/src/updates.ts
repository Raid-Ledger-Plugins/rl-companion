import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Mirrors `update_policy::UpdateStatus` (phase-tagged, flattened). */
export type UpdatePhase =
  | { phase: "idle" }
  | { phase: "checking" }
  | { phase: "upToDate" }
  | { phase: "downloading"; version: string; percent: number | null }
  | { phase: "ready"; version: string }
  | { phase: "installing"; version: string }
  | { phase: "error"; message: string };

export type UpdateStatus = UpdatePhase & {
  currentVersion: string;
  channel: "stable" | "test";
  lastChecked: string | null;
};

export const getUpdateStatus = () => invoke<UpdateStatus>("get_update_status");
export const checkForUpdates = () => invoke<void>("check_for_updates");
export const installUpdate = () => invoke<void>("install_update");

export const onUpdateStatus = (cb: (s: UpdateStatus) => void): Promise<UnlistenFn> =>
  listen<UpdateStatus>("update-status", (event) => cb(event.payload));

/** The section's one-line state. */
export function updateLine(s: UpdateStatus): string {
  switch (s.phase) {
    case "idle":
      return "Not checked yet.";
    case "checking":
      return "Checking for updates…";
    case "upToDate":
      return "You have the latest version.";
    case "downloading":
      return `Downloading ${s.version}${s.percent === null ? "…" : ` (${s.percent}%)`}`;
    case "ready":
      return `Version ${s.version} is ready to install.`;
    case "installing":
      return `Installing ${s.version}… the Companion restarts by itself.`;
    case "error":
      return s.message;
  }
}

/** Shown under the banner: when the downloaded update installs on its own. */
export const AUTO_INSTALL_HINT =
  "It installs by itself once this window is closed and nothing is uploading, or update now.";
