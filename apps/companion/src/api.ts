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

export interface CharResult {
  /** The collector that produced this result (D12); each game view shows only its own. */
  collectorId: string;
  label: string;
  result: string;
  ok: boolean;
  at: string;
}

export interface LogLine {
  at: string;
  text: string;
  /** `null` = a device-level line (rate limited, not linked), shown in every game view. */
  collectorId: string | null;
}

export type TileState = "supported" | "plugin_off" | "not_on_server" | "app_too_old" | "unknown";

/** One game tile on the home screen (computed in Rust, `tiles.rs`). */
export interface Tile {
  /** Contract id (== collector id for the app's own collectors). */
  id: string;
  /** Panel to open; `null` for `app_too_old`. */
  collectorId: string | null;
  title: string;
  subtitle: string | null;
  state: TileState;
  /** Shown ON the art: "Plugin not installed", "Not on this server", … */
  chip: string | null;
  /** The line under the art. */
  line: string;
  /** The game view's banner (plain text). */
  banner: string | null;
  /** Bolded inside `banner`. */
  pluginName: string | null;
  clickable: boolean;
  /** A `data:` URL, or `null`. */
  cover: string | null;
  /** `true` → the gradient + initials fallback. */
  fallback: boolean;
  initials: string;
}

export interface CollectorStatus {
  enabled: boolean;
  wowDir: string;
  status: string;
  results: CharResult[];
  log: LogLine[];
  /** Device-level state (paused, not linked, rate limited), shown once. */
  device: string | null;
  /** The last scan hit a 401 and the token was cleared; the window shows it unlinked. */
  relink: boolean;
  collectors: CollectorRow[];
  tiles: Tile[];
  /** RFC 3339; set only while the server is unreachable and a cached answer is shown. */
  lastChecked: string | null;
}

export interface PartyLine {
  name: string;
  summary: string;
}

/** Local summary of what the first upload would send (nothing is sent yet). */
export interface Preview {
  collectorId: string;
  saveName: string;
  campaign: string;
  location: string;
  difficulty: string;
  savedAt: string;
  party: PartyLine[];
  thumbnail: string | null;
  waiting: number;
}

/** One BG3 campaign in the upload picker (ROK-2068). */
export interface Bg3Campaign {
  /** The save's `campaignKey` (SHA-256 hex). */
  key: string;
  /** The party leader's name, else the newest save's name. */
  name: string;
  /** RFC 3339. */
  newestSavedAt: string;
  saveCount: number;
  ticked: boolean;
  /** What `ticked` would be without the user's choice (on within 30 days). */
  defaultOn: boolean;
}

export interface CollectorRow {
  id: string;
  displayName: string;
  enabled: boolean;
  status: string;
  detectedFolder: string | null;
  folderOverride: string;
  overrideNote: string | null;
  confirmFirstUpload: boolean;
  consented: boolean;
  preview: Preview | null;
  /** BG3's campaign picker rows; empty for other games. */
  campaigns: Bg3Campaign[];
}

export const LEDGERLINK_ID = "wow-ledgerlink";
export const BG3_ID = "bg3-save";

export const getCollectorStatus = () => invoke<CollectorStatus>("get_collector_status");
export const setWowDir = (dir: string) => invoke<CollectorStatus>("set_wow_dir", { dir });
export const pickWowDir = () => invoke<CollectorStatus | null>("pick_wow_dir");
export const setLedgerlinkEnabled = (enabled: boolean) =>
  invoke<CollectorStatus>("set_ledgerlink_enabled", { enabled });
export const setCollectorEnabled = (id: string, enabled: boolean) =>
  invoke<CollectorStatus>("set_collector_enabled", { id, enabled });
export const setCollectorRoot = (id: string, dir: string) =>
  invoke<CollectorStatus>("set_collector_root", { id, dir });
export const pickCollectorRoot = (id: string) => invoke<CollectorStatus | null>("pick_collector_root", { id });
export const confirmFirstUpload = (id: string) => invoke<CollectorStatus>("confirm_first_upload", { id });
export const skipFirstUpload = (id: string) => invoke<CollectorStatus>("skip_first_upload", { id });
export const setBg3Campaign = (key: string, ticked: boolean) =>
  invoke<CollectorStatus>("set_bg3_campaign", { key, ticked });
export const uploadNow = () => invoke<void>("upload_now");
export const refreshServerConfig = () => invoke<CollectorStatus>("refresh_server_config");

export const onCollectorStatus = (cb: (s: CollectorStatus) => void): Promise<UnlistenFn> =>
  listen<CollectorStatus>("collector-status", (event) => cb(event.payload));
