import { invoke } from "@tauri-apps/api/core";

/** `DevBridgeStatusDto` (src-tauri/src/dev_bridge/commands.rs). */
export interface DevBridgeStatus {
  enabled: boolean;
  bindIp: string | null;
  port: number;
  allowlist: string[];
  probesEnabled: boolean;
  hasToken: boolean;
  addresses: string[];
  listening: string | null;
  error: string | null;
  certFingerprint: string | null;
}

export const DEV_BRIDGE_CONSENT =
  "This lets tools on your home network read this app's state and your WoW addon data. Only turn it on if you develop Raid Ledger.";

/**
 * The consent prompt, through tauri-plugin-dialog's `message` command (what the JS `confirm()` wraps).
 * Resolves true only on an explicit OK — Cancel or closing the dialog is false. `window.confirm` is not
 * used: Tauri v2 webviews do not reliably implement it, and the dialog plugin replaces it with an async
 * function whose Promise is always truthy.
 */
export const confirmDevBridgeConsent = async (): Promise<boolean> =>
  (await invoke<string>("plugin:dialog|message", {
    message: DEV_BRIDGE_CONSENT, title: "Turn on the developer bridge?", kind: "warning", buttons: "OkCancel",
  })) === "Ok";

/** Turning on needs consent first; turning off never asks. Returns the command promise, or null if declined. */
export async function toggleWithConsent(
  on: boolean,
  confirm: () => Promise<boolean>,
  setEnabled: (on: boolean) => Promise<DevBridgeStatus>,
): Promise<DevBridgeStatus | null> {
  if (on && (await confirm()) !== true) return null;
  return setEnabled(on);
}

export const getDevBridgeStatus = () => invoke<DevBridgeStatus>("get_dev_bridge_status");
export const setDevBridgeEnabled = (enabled: boolean) => invoke<DevBridgeStatus>("set_dev_bridge_enabled", { enabled });
export const setDevBridgeProbesEnabled = (enabled: boolean) =>
  invoke<DevBridgeStatus>("set_dev_bridge_probes_enabled", { enabled });
export const setDevBridgeBind = (ip: string, port: number) => invoke<DevBridgeStatus>("set_dev_bridge_bind", { ip, port });
export const setDevBridgeAllowlist = (ips: string[]) => invoke<DevBridgeStatus>("set_dev_bridge_allowlist", { ips });
/** Create == first Rotate; the raw token comes back exactly once. */
export const rotateDevBridgeToken = () => invoke<{ token: string }>("rotate_dev_bridge_token");

/** One IPv4 per line or comma; blanks dropped. */
export const parseAllowlist = (raw: string) =>
  raw.split(/[\s,]+/).map((s) => s.trim()).filter((s) => s.length > 0);
