export interface KeyboardShortcut {
  modifiers: Array<"cmd" | "ctrl" | "shift" | "option" | "alt">;
  key: string;
}

export const Keyboard = {
  Shortcut: {
    Common: {
      Copy: { modifiers: ["cmd"], key: "c" },
      Open: { modifiers: ["cmd"], key: "o" },
      Save: { modifiers: ["cmd"], key: "s" },
      Close: { modifiers: ["cmd"], key: "w" },
    } satisfies Record<string, KeyboardShortcut>,
  },
} as const;
