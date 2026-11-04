import { describe as suite, expect, it } from "vitest";
import { describe, importBody } from "./importing";

suite("importing", () => {
  it("leaves empty fields out of the request", () => {
    expect(importBody("curl x", "", " ", "", [])).toEqual({ content: "curl x" });
    expect(importBody("{}", "postman", " shop ", "Shop", ["{}"])).toEqual({
      content: "{}",
      format: "postman",
      id: "shop",
      name: "Shop",
      environments: ["{}"],
    });
  });

  it("describes what was imported", () => {
    const s = { id: "a", format: "hurl", name: "A", requests: 0, folders: 1, flows: 1, environments: ["dev"], secrets: 0, warnings: [] } as const;
    expect(describe({ ...s, environments: [...s.environments], warnings: [] })).toBe("0 requests, 1 folder, 1 flow, 1 environment");
  });
});
