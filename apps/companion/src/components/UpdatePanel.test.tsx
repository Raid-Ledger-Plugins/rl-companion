import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { hosts, must, text, type Host } from "../test/elements";
import { updateLine, type UpdatePhase, type UpdateStatus } from "../updates";
import { UpdateBanner, UpdateSection } from "./UpdatePanel";

const base = { currentVersion: "0.1.0-test.5", channel: "test" as const, lastChecked: null };
const st = (p: UpdatePhase): UpdateStatus => ({ ...base, ...p });
const button = (els: Host[]) => must(els.find((e) => e.type === "button"), "button");

describe("UpdateBanner", () => {
  it("offers Update now once a newer version is downloaded", () => {
    const onInstall = vi.fn();
    const els = hosts(<UpdateBanner status={st({ phase: "ready", version: "0.1.0-test.6" })} onCheck={vi.fn()} onInstall={onInstall} />);
    const html = renderToStaticMarkup(<UpdateBanner status={st({ phase: "ready", version: "0.1.0-test.6" })} onCheck={vi.fn()} onInstall={onInstall} />);
    expect(html).toContain("Version 0.1.0-test.6 is available.");
    expect(html).toContain("installs by itself once this window is closed");
    expect(text(button(els))).toBe("Update now");
    (button(els).props.onClick as () => void)();
    expect(onInstall).toHaveBeenCalledOnce();
  });

  it("shows progress while downloading and nothing when up to date", () => {
    const dl = renderToStaticMarkup(<UpdateBanner status={st({ phase: "downloading", version: "0.1.0-test.6", percent: 40 })} onCheck={vi.fn()} onInstall={vi.fn()} />);
    expect(dl).toContain("Downloading 0.1.0-test.6 (40%)");
    for (const p of [{ phase: "idle" }, { phase: "upToDate" }, { phase: "checking" }] as UpdatePhase[]) {
      expect(renderToStaticMarkup(<UpdateBanner status={st(p)} onCheck={vi.fn()} onInstall={vi.fn()} />)).toBe("");
    }
  });
});

describe("UpdateSection", () => {
  it("shows the version and channel and checks on click", () => {
    const onCheck = vi.fn();
    const els = hosts(<UpdateSection status={st({ phase: "upToDate" })} onCheck={onCheck} onInstall={vi.fn()} />);
    const html = renderToStaticMarkup(<UpdateSection status={st({ phase: "upToDate" })} onCheck={onCheck} onInstall={vi.fn()} />);
    expect(html).toContain("Version 0.1.0-test.5 · test builds");
    expect(html).toContain("You have the latest version.");
    expect(text(button(els))).toBe("Check for updates");
    (button(els).props.onClick as () => void)();
    expect(onCheck).toHaveBeenCalledOnce();
  });

  it("turns into Update now when ready and is disabled while busy", () => {
    const onInstall = vi.fn();
    const ready = button(hosts(<UpdateSection status={st({ phase: "ready", version: "0.1.0-test.6" })} onCheck={vi.fn()} onInstall={onInstall} />));
    expect(text(ready)).toBe("Update now");
    (ready.props.onClick as () => void)();
    expect(onInstall).toHaveBeenCalledOnce();
    for (const p of [
      { phase: "checking" },
      { phase: "downloading", version: "x", percent: null },
      { phase: "installing", version: "x" },
    ] as UpdatePhase[]) {
      expect(button(hosts(<UpdateSection status={st(p)} onCheck={vi.fn()} onInstall={vi.fn()} />)).props.disabled).toBe(true);
    }
  });

  it("shows a failed check as an error and lets the user retry", () => {
    const s = st({ phase: "error", message: "Could not check for updates: offline" });
    const els = hosts(<UpdateSection status={s} onCheck={vi.fn()} onInstall={vi.fn()} />);
    expect(must(els.find((e) => e.props.className === "error"), "error line")).toBeTruthy();
    expect(button(els).props.disabled).toBe(false);
    expect(updateLine(s)).toBe("Could not check for updates: offline");
  });

  it("labels stable builds", () => {
    const html = renderToStaticMarkup(<UpdateSection status={{ ...st({ phase: "idle" }), channel: "stable", currentVersion: "0.1.0" }} onCheck={vi.fn()} onInstall={vi.fn()} />);
    expect(html).toContain("Version 0.1.0 · stable builds");
    expect(html).toContain("Not checked yet.");
  });
});
