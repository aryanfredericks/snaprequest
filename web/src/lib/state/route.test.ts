import { describe, expect, it } from "vitest";
import { formatRoute, parseRoute, type Route } from "./route";

describe("routes", () => {
  it("round-trips", () => {
    const routes: Route[] = [
      { kind: "request", collection: "shop", key: "orders/create order" },
      { kind: "flow", collection: "my shop", key: "orders/checkout" },
      { kind: "environment", collection: "shop", name: "dev" },
      { kind: "collection", collection: "shop" },
      { kind: "folder", collection: "shop", key: "orders/admin" },
      { kind: "history", id: 42 },
      { kind: "run", id: 7 },
      { kind: "new" },
      { kind: "home" },
    ];
    for (const r of routes) expect(parseRoute(formatRoute(r))).toEqual(r);
    expect(formatRoute(routes[0]!)).toBe("#/c/shop/r/orders/create%20order");
  });

  it("falls back to home", () => {
    expect(parseRoute("")).toEqual({ kind: "home" });
    expect(parseRoute("#/history/x")).toEqual({ kind: "home" });
    expect(parseRoute("#/c/shop/r")).toEqual({ kind: "home" });
  });
});
