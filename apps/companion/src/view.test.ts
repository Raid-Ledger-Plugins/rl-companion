import { describe, expect, it } from "vitest";
import { escapeGoesBack, type KeyEventLike } from "./view";

const key = (over: Partial<KeyEventLike> = {}): KeyEventLike => ({
  key: "Escape",
  isComposing: false,
  defaultPrevented: false,
  target: { tagName: "BUTTON" },
  ...over,
});

describe("escapeGoesBack", () => {
  it("goes back on Escape outside a field", () => {
    expect(escapeGoesBack(key())).toBe(true);
    expect(escapeGoesBack(key({ target: null }))).toBe(true);
  });

  it("ignores other keys", () => {
    expect(escapeGoesBack(key({ key: "Enter" }))).toBe(false);
  });

  it.each(["INPUT", "TEXTAREA", "SELECT", "input"])("stays while a %s has focus", (tagName) => {
    expect(escapeGoesBack(key({ target: { tagName } }))).toBe(false);
  });

  it("stays in a contenteditable element", () => {
    expect(escapeGoesBack(key({ target: { tagName: "DIV", isContentEditable: true } }))).toBe(false);
  });

  it("stays during IME composition or when a field already handled it", () => {
    expect(escapeGoesBack(key({ isComposing: true }))).toBe(false);
    expect(escapeGoesBack(key({ defaultPrevented: true }))).toBe(false);
  });
});
