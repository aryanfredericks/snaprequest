import { describe, expect, it } from "vitest";
import type { HistoryRecord } from "../api/types";
import { comparableBody, comparableHeaders, draftFromHistory, timeAgo } from "./history";

const record: HistoryRecord = {
  id: 7,
  created_at: 0,
  method: "POST",
  url: "http://x/login?a=1",
  status: 200,
  duration_ms: 3,
  error: null,
  collection: null,
  request_key: null,
  request: {
    method: "POST",
    url: "http://x/login?a=1",
    headers: [
      { name: "Accept", value: "application/json" },
      { name: "Authorization", value: "<redacted>" },
    ],
    body: { type: "json", content: '{"user":"demo"}' },
    auth: "basic",
  },
  response: null,
};

describe("history", () => {
  it("rebuilds a request without the credentials it never stored", () => {
    const { draft, redacted } = draftFromHistory(record);
    expect(draft.method).toBe("POST");
    expect(draft.url).toBe("http://x/login?a=1");
    expect(draft.headers.map((h) => [h.name, h.value, h.enabled])).toEqual([
      ["Accept", "application/json", true],
      ["Authorization", "", false],
    ]);
    expect(draft.body.type).toBe("json");
    expect(draft.body.content).toBe('{"user":"demo"}');
    expect(redacted).toEqual(["Authorization", "basic auth"]);
  });

  it("formats times and comparable text", () => {
    const now = Date.parse("2026-11-03T12:00:00Z");
    expect(timeAgo(now - 10_000, now)).toBe("just now");
    expect(timeAgo(now - 5 * 60_000, now)).toBe("5 min ago");
    expect(timeAgo(now - 3 * 3600_000, now)).toBe("3 h ago");
    expect(comparableBody({ encoding: "utf8", data: '{"b":1}' }, [])).toBe('{\n  "b": 1\n}');
    expect(comparableHeaders([{ name: "X-B", value: "2" }, { name: "a", value: "1" }])).toBe("a: 1\nx-b: 2");
  });
});
