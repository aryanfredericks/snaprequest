import { describe, expect, it } from "vitest";
import { lastFormat, rememberFormat } from "./exporting";

describe("export format", () => {
  it("defaults to cURL and remembers valid choices only", () => {
    const store = new Map<string, string>();
    const storage = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v) };
    expect(lastFormat(storage)).toBe("curl");
    rememberFormat("go", storage);
    expect(lastFormat(storage)).toBe("go");
    store.set("snapreq.exportFormat", "cobol");
    expect(lastFormat(storage)).toBe("curl");
  });
});
