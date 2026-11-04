import { describe, expect, it } from "vitest";
import type { RequestFile } from "../api/types";
import { draftFromFile, draftToCore, draftToFile, emptyDraft, parseResolve, row, rowsFromTable, rowsToTable, sameFile } from "./draft";

const file: RequestFile = {
  name: "Create user",
  method: "POST",
  url: "{{base}}/users",
  vars: { role: "admin", token: { secret: true } },
  query: { tag: ["a", { value: "b", enabled: false }], page: "1" },
  headers: { Accept: "application/json" },
  auth: { type: "api_key", name: "X-Key", value: "{{key}}", location: "query" },
  body: { type: "json", content: '{ "a": 1 }' },
  settings: { timeout_ms: 500, follow_redirects: false },
  capture: { id: "$.id" },
  assert: { status: 201 },
};

describe("draft ⇄ request file", () => {
  it("round-trips a file, keeping keys the builder does not edit", () => {
    expect(draftToFile(draftFromFile(file))).toEqual(file);
  });

  it("round-trips every body type", () => {
    const bodies: RequestFile["body"][] = [
      { type: "text", content: "hi", content_type: "text/csv" },
      { type: "text", file: "data.txt" },
      { type: "json", file: "user.json" },
      { type: "form", fields: { a: "1", b: { value: "2", enabled: false } } },
      {
        type: "multipart",
        parts: [
          { name: "title", value: "x" },
          { name: "photo", file: "a.png", filename: "b.png", content_type: "image/png", enabled: false },
        ],
      },
      { type: "file", file: "blob.bin", content_type: "application/octet-stream" },
    ];
    for (const body of bodies) {
      const f: RequestFile = { url: "u", body };
      expect(draftToFile(draftFromFile(f))).toEqual(f);
    }
  });

  it("round-trips auth, dropping fields of other grants", () => {
    const f: RequestFile = {
      url: "u",
      auth: { type: "oauth2", grant: "password", token_url: "t", client_id: "c", username: "u", password: "p", params: { audience: "x" } },
    };
    expect(draftToFile(draftFromFile(f))).toEqual(f);
    const d = draftFromFile(f);
    d.auth.fields.grant = "client_credentials";
    expect(draftToFile(d).auth).toEqual({ type: "oauth2", grant: "client_credentials", token_url: "t", client_id: "c", params: { audience: "x" } });
    d.auth.type = "inherit";
    expect(draftToFile(d).auth).toBeUndefined();
  });

  it("groups rows with the same name and drops unnamed rows", () => {
    const rows = [row("a", "1"), row("", "x"), row("b", "2"), row("a", "3", false)];
    expect(rowsToTable(rows)).toEqual({ a: ["1", { value: "3", enabled: false }], b: "2" });
    expect(rowsFromTable({ a: ["1", { value: "3", enabled: false }] }).map((r) => [r.name, r.value, r.enabled])).toEqual([
      ["a", "1", true],
      ["a", "3", false],
    ]);
  });

  it("detects changes", () => {
    const saved = draftToFile(draftFromFile(file));
    const d = draftFromFile(file);
    expect(sameFile(draftToFile(d), saved)).toBe(true);
    d.headers.push(row("X-New", "1"));
    expect(sameFile(draftToFile(d), saved)).toBe(false);
  });
});

describe("draft → core request", () => {
  it("builds a core request", () => {
    const d = draftFromFile(file);
    expect(draftToCore(d)).toEqual({
      method: "POST",
      url: "{{base}}/users",
      query: [
        { name: "tag", value: "a" },
        { name: "tag", value: "b", enabled: false },
        { name: "page", value: "1" },
      ],
      headers: [{ name: "Accept", value: "application/json" }],
      body: { type: "json", content: '{ "a": 1 }' },
      auth: { type: "api_key", name: "X-Key", value: "{{key}}", location: "query" },
      settings: { timeout_ms: 500, follow_redirects: false },
    });
  });

  it("treats inherit as no auth and 0 as no timeout", () => {
    const d = emptyDraft(true);
    d.url = " http://x ";
    d.settings.timeout_ms = 0;
    d.settings.verify_tls = false;
    expect(draftToCore(d)).toEqual({ method: "GET", url: "http://x", settings: { timeout_ms: null, tls: { verify: false } } });
  });
});

describe("connection and TLS settings", () => {
  it("map to the core request and parse --resolve lines", () => {
    const d = emptyDraft();
    d.url = "https://api.test";
    d.settings = { proxy: "socks5h://127.0.0.1:1080", ca_cert: "ca.pem", client_cert: "c.pem", client_key: "k.pem", tls_min_version: "1.3" };
    d.resolve = "api.test:443:127.0.0.1\nb.test:*:[::1],10.0.0.2";
    expect(draftToCore(d).settings).toEqual({
      proxy: "socks5h://127.0.0.1:1080",
      tls: { ca_cert: "ca.pem", client_cert: { cert: "c.pem", key: "k.pem" }, min_version: "1.3" },
      resolve: [
        { host: "api.test", port: 443, addrs: ["127.0.0.1"] },
        { host: "b.test", addrs: ["::1", "10.0.0.2"] },
      ],
    });
    // Request files have no `resolve`.
    expect(draftToFile(d).settings).toEqual({ proxy: "socks5h://127.0.0.1:1080", ca_cert: "ca.pem", client_cert: "c.pem", client_key: "k.pem", tls_min_version: "1.3" });
    expect(() => parseResolve("nope")).toThrow(/HOST:PORT:ADDR/);
  });
});

describe("scripts", () => {
  it("are kept through the editor and empty ones are dropped", () => {
    const file = { url: "{{base}}/x", script: { pre_request: "snapreq.variables.set('a', 1)", test_file: "t.js" } };
    const d = draftFromFile(file);
    expect(draftToFile(d).script).toEqual(file.script);
    d.rest.script = { pre_request: "  ", test: "" };
    expect(draftToFile(d).script).toBeUndefined();
  });
});
