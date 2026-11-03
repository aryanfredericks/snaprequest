import { describe, expect, it } from "vitest";
import { scopeFromFile, scopeToFile, validVarName, varsFromTable, varsToTable } from "./scope";

describe("scope settings", () => {
  it("round-trips a collection file", () => {
    const f = {
      format: 1,
      name: "Shop",
      order: ["health", "auth"],
      vars: { base_url: "http://x", password: { secret: true as const } },
      headers: { Accept: "application/json" },
      auth: { type: "bearer" as const, token: "{{token}}" },
      settings: { timeout_ms: 1000 },
    };
    expect(scopeToFile(scopeFromFile(f, true), true)).toEqual(f);
  });

  it("treats a collection without auth as none and folders as inherit", () => {
    const c = scopeFromFile({ format: 1, name: "C" }, true);
    expect(c.auth.type).toBe("none");
    expect(scopeToFile(c, true).auth).toBeUndefined();
    const f = scopeFromFile({}, false);
    expect(f.auth.type).toBe("inherit");
    expect(scopeToFile(f, false)).toEqual({});
  });

  it("writes secrets as markers only", () => {
    const rows = varsFromTable({ a: "1", t: { secret: true } });
    rows[1]!.value = "typed but never written";
    expect(varsToTable(rows)).toEqual({ a: "1", t: { secret: true } });
    expect(validVarName("api.key")).toBe(true);
    expect(validVarName("$uuid")).toBe(false);
  });
});
