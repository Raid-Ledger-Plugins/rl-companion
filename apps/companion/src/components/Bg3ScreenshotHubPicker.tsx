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

function HubSelect({ hubs, onPick }: { hubs: Bg3ScreenshotHubs; onPick: Bg3ScreenshotHubPickerProps["onPick"] }) {
  return (
    <label>
      Send screenshots to
      <select
        value={hubs.selected === null ? "" : String(hubs.selected)}
        onChange={(e) => onPick(e.target.value === "" ? null : Number(e.target.value))}
      >
        {hubs.selected === null && <option value="">Choose a hub…</option>}
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
 * ROK-1966 Q3: which BG3 hub screenshots go to. Defaults to the hub of the
 * user's latest save (the backend resolves it). Hook-free.
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
      {listed && hubs.targets.length > 0 && hubs.selected === null && (
        <span className="muted">Pick a hub to start sending screenshots.</span>
      )}
      {waiting && <span role="status" className="muted">{waiting}</span>}
    </div>
  );
}
