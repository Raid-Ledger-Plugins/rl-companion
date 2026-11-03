/** D8: the window shows the home screen or one game's settings. No router. */
export type View = { kind: "home" } | { kind: "game"; id: string };

/** The only things that move the view. A `collector-status` event is not one of them. */
export type NavAction = { type: "open"; id: string } | { type: "back" } | { type: "key"; key: string };

export const HOME: View = { kind: "home" };

export function navigate(view: View, action: NavAction): View {
  switch (action.type) {
    case "open":
      return { kind: "game", id: action.id };
    case "back":
      return HOME;
    case "key":
      return action.key === "Escape" ? HOME : view;
  }
}

/** The parts of a `keydown` event the Escape rule reads. */
export interface KeyEventLike {
  key: string;
  isComposing: boolean;
  defaultPrevented: boolean;
  target: unknown;
}

const EDITABLE = new Set(["INPUT", "TEXTAREA", "SELECT"]);

function isEditable(t: unknown): boolean {
  if (typeof t !== "object" || t === null) return false;
  const tag = "tagName" in t && typeof t.tagName === "string" ? t.tagName.toUpperCase() : "";
  return EDITABLE.has(tag) || ("isContentEditable" in t && t.isContentEditable === true);
}

/**
 * Escape leaves the game view, except while the user is typing: Escape in a
 * text field (or ending an IME composition) means "cancel this field".
 */
export function escapeGoesBack(e: KeyEventLike): boolean {
  return e.key === "Escape" && !e.isComposing && !e.defaultPrevented && !isEditable(e.target);
}
