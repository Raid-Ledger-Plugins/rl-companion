import { describe, expect, it, vi } from "vitest";
import { hosts, must, text, type Host } from "../test/elements";
import { parseAllowlist, type DevBridgeStatus } from "../devBridge";
import { DevBridgePanel, type DevBridgePanelProps } from "./DevBridgePanel";

const OFF: DevBridgeStatus = {
  enabled: false, bindIp: null, port: 47655, allowlist: [], probesEnabled: false, hasToken: false,
  addresses: ["192.168.0.10"], listening: null, error: null, certFingerprint: null,
};
const ON: DevBridgeStatus = { ...OFF, enabled: true, bindIp: "192.168.0.10", hasToken: true, listening: "192.168.0.10:47655" };

function props(over: Partial<DevBridgePanelProps> = {}): DevBridgePanelProps {
  return {
    status: OFF, drafts: { ip: "", port: "47655", allowlist: "" }, token: null,
    onDrafts: vi.fn(), onToggle: vi.fn(), onProbes: vi.fn(), onSaveBind: vi.fn(), onSaveAllowlist: vi.fn(),
    onRotate: vi.fn(), onCopy: vi.fn(), onDismissToken: vi.fn(), ...over,
  };
}
const named = (els: Host[], name: string) => must(els.find((e) => e.props.name === name), name);
const has = (els: Host[], name: string) => els.some((e) => e.props.name === name);

describe("DevBridgePanel", () => {
  it("is collapsed and off by default; probes cannot be allowed while the bridge is off", () => {
    const els = hosts(<DevBridgePanel {...props()} />);
    expect(els[0]?.type).toBe("details");
    expect(named(els, "bridge").props.checked).toBe(false);
    expect(named(els, "probes").props.disabled).toBe(true);
    expect(has(els, "ip") || has(els, "rotate")).toBe(false);
  });

  it("toggling calls onToggle with the new value", () => {
    const onToggle = vi.fn();
    const els = hosts(<DevBridgePanel {...props({ onToggle })} />);
    (named(els, "bridge").props.onChange as (e: unknown) => void)({ target: { checked: true } });
    expect(onToggle).toHaveBeenCalledWith(true);
  });

  it("while on: shows the listener, the address list and Rotate (Create before a token exists)", () => {
    const els = hosts(<DevBridgePanel {...props({ status: ON })} />);
    expect(els.map(text).join(" ")).toContain("Dev bridge on — listening on 192.168.0.10:47655");
    expect(text(named(els, "rotate"))).toBe("Rotate token");
    expect(named(els, "probes").props.disabled).toBe(false);
    const none = hosts(<DevBridgePanel {...props({ status: { ...ON, hasToken: false } })} />);
    expect(text(named(none, "rotate"))).toBe("Create token");
  });

  it("shows a rotated token only while `token` is set, with Copy", () => {
    const onCopy = vi.fn();
    const shown = hosts(<DevBridgePanel {...props({ status: ON, token: "rlcdev_abc", onCopy })} />);
    expect(shown.some((e) => text(e) === "rlcdev_abc")).toBe(true);
    (named(shown, "copy").props.onClick as () => void)();
    expect(onCopy).toHaveBeenCalledOnce();
    const hidden = hosts(<DevBridgePanel {...props({ status: ON })} />);
    expect(hidden.some((e) => text(e).includes("rlcdev_"))).toBe(false);
  });

  it("parses the allowed-callers box", () => {
    expect(parseAllowlist(" 192.168.0.229\n\n10.0.0.2, 10.0.0.3 ")).toEqual(["192.168.0.229", "10.0.0.2", "10.0.0.3"]);
  });
});
