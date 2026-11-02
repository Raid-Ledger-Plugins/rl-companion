import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { hosts, must, type Host } from "../test/elements";
import { CLOSE_HINT, setStartAtLogin, type AutostartApi } from "../startup";
import { StartupPanel, type StartupPanelProps } from "./StartupPanel";

const tree = (p: StartupPanelProps) => hosts(<StartupPanel {...p} />);
const checkbox = (els: Host[]) => must(els.find((e) => e.props.type === "checkbox"), "checkbox");

function fakeApi(initial: boolean): AutostartApi & { state: { on: boolean } } {
  const state = { on: initial };
  return {
    state,
    enable: vi.fn(async () => { state.on = true; }),
    disable: vi.fn(async () => { state.on = false; }),
    isEnabled: vi.fn(async () => state.on),
  };
}

describe("StartupPanel — Start at login", () => {
  it("reflects the OS state and calls onToggle with the new value", () => {
    const onToggle = vi.fn();
    const off = tree({ enabled: false, onToggle });
    expect(checkbox(off).props.checked).toBe(false);
    expect(checkbox(off).props.disabled).toBe(false);
    (checkbox(off).props.onChange as (e: unknown) => void)({ target: { checked: true } });
    expect(onToggle).toHaveBeenCalledWith(true);

    expect(checkbox(tree({ enabled: true, onToggle })).props.checked).toBe(true);
  });

  it("is disabled until the state has loaded", () => {
    expect(checkbox(tree({ enabled: null, onToggle: vi.fn() })).props.disabled).toBe(true);
  });

  it("tells the user that closing the window keeps it in the tray", () => {
    const html = renderToStaticMarkup(<StartupPanel enabled={true} onToggle={vi.fn()} />);
    expect(html).toContain("Start at login");
    expect(html).toContain("running in the tray (^ by the clock)");
    expect(CLOSE_HINT).toBe(
      "Closing this window keeps the Companion running in the tray (^ by the clock). Quit from the tray menu.",
    );
  });
});

describe("setStartAtLogin", () => {
  it("turning it off calls disable and reports the OS state", async () => {
    const api = fakeApi(true);
    await expect(setStartAtLogin(false, api)).resolves.toBe(false);
    expect(api.disable).toHaveBeenCalledOnce();
    expect(api.enable).not.toHaveBeenCalled();
  });

  it("turning it on calls enable and reports the OS state", async () => {
    const api = fakeApi(false);
    await expect(setStartAtLogin(true, api)).resolves.toBe(true);
    expect(api.enable).toHaveBeenCalledOnce();
    expect(api.disable).not.toHaveBeenCalled();
  });
});
