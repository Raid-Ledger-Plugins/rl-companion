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
