import type { Bg3ScreenshotHubs } from "../api";

export interface Bg3ScreenshotHubPickerProps {
  /** `null` until the first `get_bg3_screenshot_hubs` answers. */
  hubs: Bg3ScreenshotHubs | null;
  onPick: (hubGuildId: number | null) => void;
}

function waitingLine(n: number): string | null {
  if (n <= 0) return null;
  return n === 1 ? "1 screenshot is waiting for a hub." : `${n} screenshots are waiting for a hub.`;
}

/** A pick the server no longer lists (left hub, deleted hub): the backend reports it as `stalePick`. */
export function isStale(hubs: Bg3ScreenshotHubs): boolean {
  return hubs.stalePick !== null;
}

/**
 * The select's value: the pinned hub, or "" (the first option) while following
 * the latest save — or while a stale pick routes uploads to that default (r2-m2).
 */
function selectValue(hubs: Bg3ScreenshotHubs): string {
  if (hubs.followingLatestSave || isStale(hubs) || hubs.selected === null) return "";
  return String(hubs.selected);
}

/** The first option: follow the latest save's hub (m6), or a placeholder when there is none. */
function firstLabel(hubs: Bg3ScreenshotHubs): string {
  const def = hubs.targets.find((t) => t.isDefault);
  return def ? `Follow my latest save (${def.name})` : "Choose a hub…";
}

function HubSelect({ hubs, onPick }: { hubs: Bg3ScreenshotHubs; onPick: Bg3ScreenshotHubPickerProps["onPick"] }) {
  return (
    <label>
      Send screenshots to
      <select
        value={selectValue(hubs)}
        onChange={(e) => onPick(e.target.value === "" ? null : Number(e.target.value))}
      >
        <option value="">{firstLabel(hubs)}</option>
        {hubs.targets.map((t) => (
          <option key={t.hubGuildId} value={String(t.hubGuildId)}>
            {t.isDefault ? `${t.name} (your latest save)` : t.name}
          </option>
        ))}
      </select>
    </label>
  );
}

/**
 * ROK-1966 Q3: which BG3 hub screenshots go to. Defaults to following the hub
 * of the user's latest save (the backend resolves it); picking a hub pins it,
 * picking "Follow my latest save" clears the pin. Hook-free.
 */
export function Bg3ScreenshotHubPicker({ hubs, onPick }: Bg3ScreenshotHubPickerProps) {
  const waiting = hubs ? waitingLine(hubs.waiting) : null;
  const listed = hubs?.serverListsHubs === true;
  return (
    <div data-slot="bg3-screenshot-hub-picker" className="camps-box">
      <span className="muted small">Only screenshots you take after turning this on are sent. Older ones stay on this computer.</span>
      {hubs && !listed && (
        <span className="muted">Your Raid Ledger server doesn't list hubs for screenshots yet, so nothing is sent for now.</span>
      )}
      {listed && hubs.targets.length === 0 && (
        <span className="muted">You're not in a Baldur's Gate 3 hub yet. Join one on Raid Ledger to send screenshots.</span>
      )}
      {listed && hubs.targets.length > 0 && <HubSelect hubs={hubs} onPick={onPick} />}
      {listed && isStale(hubs) && (
        <span role="alert" className="error">You're no longer in the hub you picked. Pick another to keep sending screenshots.</span>
      )}
      {listed && hubs.targets.length > 0 && hubs.selected === null && (
        <span className="muted">Pick a hub to start sending screenshots.</span>
      )}
      {waiting && <span role="status" className="muted">{waiting}</span>}
    </div>
  );
}
