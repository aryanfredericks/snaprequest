import { describe, expect, it } from "vitest";
import type { VariableInfo } from "../api/types";
import { describeVar, placeholders, varState } from "./vars";

const vars: VariableInfo[] = [
  { name: "base", value: "http://x", secret: false, has_value: true, scope: "environment `dev`" },
  { name: "token", value: null, secret: true, has_value: true, scope: "collection" },
];

describe("placeholders", () => {
  it("finds placeholders, with spaces, skipping escaped ones", () => {
    expect(placeholders("{{base}}/u/{{ id }}?x=\\{{lit}}&t={{$uuid}}")).toEqual([
      { from: 0, to: 8, name: "base" },
      { from: 11, to: 19, name: "id" },
      { from: 33, to: 42, name: "$uuid" },
    ]);
  });

  it("resolves names against the visible variables", () => {
    expect(varState("base", vars)).toBe("defined");
    expect(varState("token", vars)).toBe("secret");
    expect(varState("$uuid", vars)).toBe("dynamic");
    expect(varState("nope", vars)).toBe("undefined");
    expect(varState("nope", null)).toBe("unknown");
    expect(describeVar("base", vars)).toBe("base = http://x (environment `dev`)");
    expect(describeVar("token", vars)).toBe("token: secret (set) from collection");
  });
});
