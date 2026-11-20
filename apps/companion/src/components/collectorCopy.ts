import { ADDON_ERRORS_ID, BG3_SCREENSHOT_ID, type CollectorRow } from "../api";

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
  if (row.id === ADDON_ERRORS_ID) {
    return {
      heading: "World of Warcraft: addon errors",
      toggle: "Share addon errors (BugGrabber / BugSack) with Raid Ledger",
      offNote:
        "Off until you turn it on. While off, your BugGrabber file is not read. When on, Raid Ledger receives each error's text and stack trace, with folder paths and your own account, character and realm names removed, plus when and how often it happened, the BugGrabber version, your WoW flavour and a random-keyed code that only this computer can link to your account (never the account name itself). Other players' names that appear in an error are not removed.",
      folderLabel: "World of Warcraft folder",
      folderPlaceholder: "Choose the World of Warcraft folder",
      resultsHeading: "Recent addon error uploads",
    };
  }
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
