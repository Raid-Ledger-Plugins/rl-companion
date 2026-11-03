import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

const { confirmDevBridgeConsent, toggleWithConsent, DEV_BRIDGE_CONSENT } = await import("./devBridge");

describe("dev bridge consent", () => {
  beforeEach(() => invoke.mockReset());

  it("asks through the dialog plugin's message command and is true only on Ok", async () => {
    invoke.mockResolvedValueOnce("Ok");
    expect(await confirmDevBridgeConsent()).toBe(true);
    expect(invoke).toHaveBeenCalledWith("plugin:dialog|message", expect.objectContaining({
      message: DEV_BRIDGE_CONSENT, buttons: "OkCancel",
    }));
    invoke.mockResolvedValueOnce("Cancel");
    expect(await confirmDevBridgeConsent()).toBe(false);
  });

  it("cancel → the enable command is not invoked", async () => {
    const setEnabled = vi.fn();
    const result = await toggleWithConsent(true, () => Promise.resolve(false), setEnabled);
    expect(result).toBeNull();
    expect(setEnabled).not.toHaveBeenCalled();
  });

  it("an unresolved-but-truthy answer is not consent; only `true` enables", async () => {
    const setEnabled = vi.fn();
    const truthy = () => Promise.resolve("Cancel" as unknown as boolean);
    expect(await toggleWithConsent(true, truthy, setEnabled)).toBeNull();
    expect(setEnabled).not.toHaveBeenCalled();
  });

  it("OK enables; turning off never asks", async () => {
    const status = { enabled: true } as never;
    const setEnabled = vi.fn().mockResolvedValue(status);
    const confirm = vi.fn().mockResolvedValue(true);
    expect(await toggleWithConsent(true, confirm, setEnabled)).toBe(status);
    expect(setEnabled).toHaveBeenCalledWith(true);
    const never = vi.fn();
    await toggleWithConsent(false, never, setEnabled);
    expect(never).not.toHaveBeenCalled();
    expect(setEnabled).toHaveBeenLastCalledWith(false);
  });
});
