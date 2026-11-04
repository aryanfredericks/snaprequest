import { describe, expect, it } from "vitest";
import { ApiError, createApi, onUnauthorized, serverVersion, unwrap } from "./client";

function fakeFetch(status: number, body: unknown, seen: Request[] = []): typeof fetch {
  return async (input: RequestInfo | URL, init?: RequestInit) => {
    seen.push(input instanceof Request ? input : new Request(input, init));
    return new Response(JSON.stringify(body), {
      status,
      headers: { "content-type": "application/json" },
    });
  };
}

describe("api client", () => {
  it("reads the server version", async () => {
    const seen: Request[] = [];
    const api = createApi("http://snapreq.test", fakeFetch(200, { name: "snapreq", version: "0.1.0", api: 1 }, seen));
    expect(await serverVersion(api)).toEqual({ name: "snapreq", version: "0.1.0", api: 1 });
    expect(seen[0]?.url).toBe("http://snapreq.test/api/v1/version");
  });

  it("turns error bodies into ApiError", async () => {
    const api = createApi(
      "http://snapreq.test",
      fakeFetch(404, { error: { kind: "not_found", message: "no history entry 7" } }),
    );
    const call = api.GET("/api/v1/history/{id}", { params: { path: { id: 7 } } }).then(unwrap);
    await expect(call).rejects.toMatchObject({ status: 404, kind: "not_found", message: "no history entry 7" });
    await expect(call).rejects.toBeInstanceOf(ApiError);
  });

  it("sends typed query parameters", async () => {
    const seen: Request[] = [];
    const api = createApi("http://snapreq.test", fakeFetch(200, { entries: [] }, seen));
    const list = unwrap(await api.GET("/api/v1/history", { params: { query: { method: "GET", limit: 5 } } }));
    expect(list.entries).toEqual([]);
    const url = new URL(seen[0]!.url);
    expect(url.pathname).toBe("/api/v1/history");
    expect(url.searchParams.get("method")).toBe("GET");
    expect(url.searchParams.get("limit")).toBe("5");
  });

  it("sends the CSRF header and reports ended sessions", async () => {
    let ended = 0;
    onUnauthorized(() => ended++);
    const seen: Request[] = [];
    const api = createApi(
      "http://snapreq.test",
      fakeFetch(401, { error: { kind: "unauthorized", message: "log in" } }, seen),
    );
    await api.DELETE("/api/v1/history");
    expect(seen[0]?.headers.get("x-snapreq-csrf")).toBe("1");
    expect(ended).toBe(1);
    // A wrong password at the login is not an ended session.
    await api.POST("/api/v1/auth/login", { body: { username: "a", password: "b" } });
    expect(ended).toBe(1);
    onUnauthorized(() => {});
  });
});
