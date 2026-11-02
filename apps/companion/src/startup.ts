import * as autostart from "@tauri-apps/plugin-autostart";

/** The slice of `@tauri-apps/plugin-autostart` the window uses (injectable for tests). */
export interface AutostartApi {
  enable: () => Promise<void>;
  disable: () => Promise<void>;
  isEnabled: () => Promise<boolean>;
}

export const autostartApi: AutostartApi = autostart;

/** Turn start-at-login on or off, then report what the OS actually has. */
export async function setStartAtLogin(on: boolean, api: AutostartApi = autostartApi): Promise<boolean> {
  await (on ? api.enable() : api.disable());
  return api.isEnabled();
}

/** Shown under the switch: closing the window does not quit (ROK-2065). */
export const CLOSE_HINT =
  "Closing this window keeps the Companion running in the tray (^ by the clock). Quit from the tray menu.";
