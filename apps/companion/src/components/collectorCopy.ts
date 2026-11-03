import { BG3_SCREENSHOT_ID, type CollectorRow } from "../api";

/** The words a game panel uses for what its collector uploads (ROK-1966 m7). */
export interface CollectorCopy {
  heading: string;
  toggle: string;
  offNote: string;
  folderLabel: string;
  folderPlaceholder: string;
  resultsHeading: string;
}

/** Shared game-panel copy: saves by default, screenshots for the BG3 screenshot collector. */
export function collectorCopy(row: CollectorRow): CollectorCopy {
  if (row.id === BG3_SCREENSHOT_ID) {
    return {
      heading: "Baldur's Gate 3: screenshot uploads",
      toggle: "Upload new Baldur's Gate 3 screenshots automatically",
      offNote: "Off until you turn it on. Nothing is read from your screenshots folder while off.",
      folderLabel: "Screenshots folder",
      folderPlaceholder: "Choose the screenshots folder",
      resultsHeading: "Recent screenshot uploads",
    };
  }
  return {
    heading: `${row.displayName}: save uploads`,
    toggle: `Upload new ${row.displayName} saves automatically`,
    offNote: "Off until you turn it on. Nothing is read from your save folder while off.",
    folderLabel: "Save folder",
    folderPlaceholder: "Choose the save folder",
    resultsHeading: "Last upload per save",
  };
}
