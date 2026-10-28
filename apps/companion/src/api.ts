import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface Status {
  serverUrl: string;
  deviceName: string;
  linked: boolean;
  tokenHint: string | null;
  paused: boolean;
}

export interface CheckResult {
  state: "unlinked" | "linked" | "revoked" | "unreachable";
  deviceName: string | null;
  message: string | null;
}

export interface LinkStart {
  userCode: string;
  verificationUri: string;
  verificationUriComplete: string;
  expiresIn: number;
}

export interface LinkEvent {
  state: "linked" | "denied" | "expired" | "invalid" | "device_limit" | "failed";
  message: string | null;
  deviceName: string | null;
}

export const getStatus = () => invoke<Status>("get_status");
export const checkLink = () => invoke<CheckResult>("check_link");
export const setServerUrl = (url: string) => invoke<string>("set_server_url", { url });
export const setDeviceName = (name: string) => invoke<string>("set_device_name", { name });
export const startLink = () => invoke<LinkStart>("start_link");
export const cancelLink = () => invoke<void>("cancel_link");
export const openVerification = (url: string) => invoke<void>("open_verification", { url });
export const unlink = () => invoke<void>("unlink");

export const onLinkEvent = (cb: (e: LinkEvent) => void): Promise<UnlistenFn> =>
  listen<LinkEvent>("link-status", (event) => cb(event.payload));

export const LINK_MESSAGES: Record<LinkEvent["state"], string> = {
  linked: "Linked.",
  denied: "The link was denied on the website. Start again if that was a mistake.",
  expired: "The code expired. Start a new link.",
  invalid: "That link is no longer valid. Start a new link.",
  device_limit: "You already have 10 linked devices. Remove one from your Devices list, then try again.",
  failed: "Linking failed.",
};
