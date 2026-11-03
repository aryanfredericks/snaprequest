import { describe, expect, it } from "vitest";
import { formatJson, formatXml } from "./format";
import { formatBytes, formatMs, hexRow, isTextual, parseSetCookie, responseCookies, statusText } from "./http";

describe("http helpers", () => {
  it("parses Set-Cookie", () => {
    expect(parseSetCookie("sid=abc=1; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=60; Domain=.x.test")).toEqual({
      name: "sid",
      value: "abc=1",
      path: "/",
      httpOnly: true,
      secure: true,
      sameSite: "Lax",
      maxAge: "60",
      domain: ".x.test",
      partitioned: false,
    });
    expect(parseSetCookie("broken")).toBeNull();
    expect(responseCookies([{ name: "Set-Cookie", value: "a=1" }, { name: "x", value: "y" }]).map((c) => c.name)).toEqual(["a"]);
  });

  it("formats sizes, times and statuses", () => {
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1536)).toBe("1.5 KiB");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MiB");
    expect(formatMs(0.5)).toBe("0.50 ms");
    expect(formatMs(12.4)).toBe("12 ms");
    expect(formatMs(1500)).toBe("1.50 s");
    expect(formatMs(null)).toBe("–");
    expect(statusText(404)).toBe("Not Found");
  });

  it("tells text from binary", () => {
    const enc = new TextEncoder();
    expect(isTextual("application/problem+json", enc.encode("{}"), true)).toBe(true);
    expect(isTextual("image/png", enc.encode("x"), false)).toBe(false);
    expect(isTextual(undefined, enc.encode("plain words"), true)).toBe(true);
    expect(isTextual(undefined, new Uint8Array([1, 2, 3]), true)).toBe(false);
  });

  it("dumps hex", () => {
    const bytes = new TextEncoder().encode("Hello, world!\n\u0001ABC");
    expect(hexRow(bytes, 0)).toEqual({
      offset: "00000000",
      hex: "48 65 6c 6c 6f 2c 20 77  6f 72 6c 64 21 0a 01 41",
      text: "Hello, world!..A",
    });
    expect(hexRow(bytes, 1).text).toBe("BC");
  });
});

describe("formatting", () => {
  it("formats JSON and leaves invalid JSON alone", () => {
    expect(formatJson('{"a":[1,2]}')).toBe('{\n  "a": [\n    1,\n    2\n  ]\n}');
    expect(formatJson("{nope")).toBe("{nope");
  });

  it("indents XML", () => {
    expect(formatXml('<?xml version="1.0"?><a><b x="1">text</b><c/><!-- note --><d><![CDATA[<raw>]]></d></a>')).toBe(
      ['<?xml version="1.0"?>', "<a>", '  <b x="1">text</b>', "  <c/>", "  <!-- note -->", "  <d>", "    <![CDATA[<raw>]]>", "  </d>", "</a>"].join("\n"),
    );
  });
});
