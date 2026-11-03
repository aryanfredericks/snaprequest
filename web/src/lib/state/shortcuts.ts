// Keyboard shortcuts: one list for the handler and the help dialog. Alt shortcuts match the
// physical key (`code`), because Alt/Option changes the typed character on many layouts.

export type Action =
  | "send"
  | "save"
  | "new"
  | "close"
  | "prev-tab"
  | "next-tab"
  | "focus-url"
  | "side-collections"
  | "side-history"
  | "side-runs"
  | "help";

export interface Shortcut {
  action: Action;
  keys: string;
  does: string;
}

export const SHORTCUTS: Shortcut[] = [
  { action: "send", keys: "Ctrl+Enter", does: "Send the request, start the run, or run the flow" },
  { action: "save", keys: "Ctrl+S", does: "Save" },
  { action: "new", keys: "Alt+N", does: "New request" },
  { action: "close", keys: "Alt+W", does: "Close the tab" },
  { action: "prev-tab", keys: "Alt+[", does: "Previous tab" },
  { action: "next-tab", keys: "Alt+]", does: "Next tab" },
  { action: "focus-url", keys: "Alt+L", does: "Go to the URL" },
  { action: "side-collections", keys: "Alt+1", does: "Show collections" },
  { action: "side-history", keys: "Alt+2", does: "Show history" },
  { action: "side-runs", keys: "Alt+3", does: "Show runs" },
  { action: "help", keys: "?", does: "Show these shortcuts (outside text fields)" },
];

type Key = Pick<KeyboardEvent, "key" | "code" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey"> & {
  /** Whether the focus is in a text field (`?` types there). */
  typing?: boolean;
};

export function match(e: Key): Action | null {
  const mod = e.ctrlKey || e.metaKey;
  if (mod && !e.altKey && e.key === "Enter") return "send";
  if (mod && !e.altKey && e.key.toLowerCase() === "s") return "save";
  if (e.altKey && !mod) {
    switch (e.code) {
      case "KeyN":
        return "new";
      case "KeyW":
        return "close";
      case "BracketLeft":
        return "prev-tab";
      case "BracketRight":
        return "next-tab";
      case "KeyL":
        return "focus-url";
      case "Digit1":
        return "side-collections";
      case "Digit2":
        return "side-history";
      case "Digit3":
        return "side-runs";
    }
  }
  if (e.key === "?" && !mod && !e.altKey && !e.typing) return "help";
  return null;
}

/** Shortcut text for this platform (⌘ on Apple systems). */
export function label(keys: string, apple = /Mac|iPhone|iPad/.test(navigator.platform)): string {
  return apple ? keys.replace("Ctrl+", "⌘").replace("Alt+", "⌥") : keys;
}
