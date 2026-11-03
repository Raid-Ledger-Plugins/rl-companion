import type { Bg3Campaign } from "../api";

/** Campaigns past this many (the oldest) fold under "Show N more". */
export const CAMPAIGN_CAP = 8;

export interface Bg3CampaignPickerProps {
  campaigns: Bg3Campaign[];
  /** BG3 uploads are on; while off nothing is scanned, so nothing is listed. */
  enabled: boolean;
  onTick: (key: string, ticked: boolean) => void;
}

/** "1 Nov", with the year when it is not this year. */
export function savedOn(iso: string, now: Date = new Date()): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "unknown";
  const year = d.getFullYear() === now.getFullYear() ? {} : { year: "numeric" as const };
  return d.toLocaleDateString(undefined, { day: "numeric", month: "short", ...year });
}

function Row({ c, onTick }: { c: Bg3Campaign; onTick: Bg3CampaignPickerProps["onTick"] }) {
  const saves = c.saveCount === 1 ? "1 save" : `${c.saveCount} saves`;
  const hint = !c.ticked && !c.defaultOn ? " (older than 30 days, off by default)" : "";
  return (
    <li>
      <label className="toggle">
        <input type="checkbox" checked={c.ticked} onChange={(e) => onTick(c.key, e.target.checked)} />
        <span>
          <b>{c.name}</b>
          <br />
          <span className="muted small">Newest save {savedOn(c.newestSavedAt)} · {saves}{hint}</span>
        </span>
      </label>
    </li>
  );
}

function List({ items, onTick }: { items: Bg3Campaign[]; onTick: Bg3CampaignPickerProps["onTick"] }) {
  return (
    <ul className="camps">
      {items.map((c) => <Row key={c.key} c={c} onTick={onTick} />)}
    </ul>
  );
}

/**
 * ROK-2068 / ROK-2067 D15: which BG3 campaigns upload. Unticking only stops
 * future uploads. Hook-free: the overflow folds into a native `<details>`.
 */
export function Bg3CampaignPicker(p: Bg3CampaignPickerProps) {
  const head = p.campaigns.slice(0, CAMPAIGN_CAP);
  const rest = p.campaigns.slice(CAMPAIGN_CAP);
  return (
    <div data-slot="bg3-campaign-picker" className="camps-box">
      {p.enabled && <h2>Campaigns to upload</h2>}
      {p.enabled && head.length === 0 && <span className="muted">Your campaigns appear here after the first scan.</span>}
      {p.enabled && head.length > 0 && <List items={head} onTick={p.onTick} />}
      {p.enabled && rest.length > 0 && (
        <details className="camps-more">
          <summary>Show {rest.length} more</summary>
          <List items={rest} onTick={p.onTick} />
        </details>
      )}
      {p.enabled && head.length > 0 && <span className="muted small">Unticking stops new uploads only. Nothing already uploaded is removed.</span>}
    </div>
  );
}
