import { describe, expect, it } from "vitest";
import { label, match } from "./shortcuts";

const key = (k: Partial<Parameters<typeof match>[0]>) =>
  match({ key: "", code: "", ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...k });

describe("shortcuts", () => {
  it("match keys and codes", () => {
    expect(key({ key: "Enter", ctrlKey: true })).toBe("send");
    expect(key({ key: "Enter", metaKey: true })).toBe("send");
    expect(key({ key: "S", ctrlKey: true, shiftKey: true })).toBe("save");
    // Option+N types "˜" on a Mac: the physical key decides.
    expect(key({ key: "˜", code: "KeyN", altKey: true })).toBe("new");
    expect(key({ key: "“", code: "BracketLeft", altKey: true })).toBe("prev-tab");
    expect(key({ key: "2", code: "Digit2", altKey: true })).toBe("side-history");
    expect(key({ key: "?", shiftKey: true })).toBe("help");
    expect(key({ key: "?", shiftKey: true, typing: true })).toBeNull();
    expect(key({ key: "Enter" })).toBeNull();
    expect(key({ key: "n", code: "KeyN", altKey: true, ctrlKey: true })).toBeNull();
  });

  it("label keys per platform", () => {
    expect(label("Ctrl+Enter", false)).toBe("Ctrl+Enter");
    expect(label("Ctrl+S", true)).toBe("⌘S");
    expect(label("Alt+N", true)).toBe("⌥N");
  });
});
