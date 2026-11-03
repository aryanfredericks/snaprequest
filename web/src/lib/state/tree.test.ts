import { describe, expect, it } from "vitest";
import { canDrop } from "./tree.svelte";

describe("drag and drop rules", () => {
  it("keeps folders out of themselves and items in their collection", () => {
    const folder = { collection: "shop", kind: "folder" as const, key: "orders" };
    expect(canDrop(folder, "shop", "")).toBe(true);
    expect(canDrop(folder, "shop", "auth")).toBe(true);
    expect(canDrop(folder, "shop", "orders")).toBe(false);
    expect(canDrop(folder, "shop", "orders/admin")).toBe(false);
    expect(canDrop(folder, "other", "")).toBe(false);
    expect(canDrop({ collection: "shop", kind: "request", key: "a" }, "shop", "orders")).toBe(true);
    expect(canDrop(null, "shop", "")).toBe(false);
  });
});
