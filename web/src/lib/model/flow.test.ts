import { describe, expect, it } from "vitest";
import type { FlowFile, RunStep } from "../api/types";
import { flowFromFile, flowToFile, newStep, stepTitle } from "./flow";
import { capturedAfter, parseData, reportCounts } from "./runs";

const checkout: FlowFile = {
  name: "Checkout",
  vars: { item: "sku-42" },
  step: [
    { request: "auth/login" },
    {
      name: "Create order",
      method: "POST",
      url: "{{base_url}}/orders",
      body: { type: "json", content: '{ "item": "{{item}}" }' },
      capture: { order_id: "$.id" },
      assert: { status: 201 },
    },
    { name: "Wait", url: "{{base_url}}/orders/{{order_id}}", retry: { max: 10, every_ms: 50, until: { json: { "$.state": "ready" } } }, assert: { status: 200 } },
    { request: "orders/get", vars: { x: "1" }, delay_ms: 100, assert: { status: 404 } },
  ],
};

describe("flow model", () => {
  it("round-trips a flow file", () => {
    expect(flowToFile(flowFromFile(checkout))).toEqual(checkout);
  });

  it("titles steps like the report", () => {
    const d = flowFromFile(checkout);
    expect(d.steps.map(stepTitle)).toEqual(["auth/login", "Create order", "Wait", "orders/get"]);
    expect(stepTitle(newStep("inline"))).toBe("GET {{base_url}}/");
  });
});

describe("runs", () => {
  it("parses CSV and JSON data", () => {
    expect(parseData('user,note\nann,"hello, ""world"""\r\nbob,\n')).toEqual([
      { user: "ann", note: 'hello, "world"' },
      { user: "bob", note: "" },
    ]);
    expect(parseData('[{"user":"ann","age":3}]')).toEqual([{ user: "ann", age: "3" }]);
    expect(() => parseData("a,b\n1")).toThrow(/row 2/);
    expect(() => parseData('{"a":1}')).toThrow();
    expect(parseData("  ")).toEqual([]);
  });

  it("accumulates captures and counts results", () => {
    const step = (number: number, captures: RunStep["captures"], passed = true): RunStep => ({
      number,
      title: `s${number}`,
      key: "k",
      passed,
      attempts: 1,
      request: null,
      response: null,
      error: null,
      assertions: [{ subject: "status", check: "equals 200", passed }],
      until: [],
      captures,
    });
    const steps = [
      step(1, [{ name: "token", value: "<redacted>", secret: true }]),
      step(2, [{ name: "id", value: "1", secret: false }]),
      step(3, [{ name: "id", value: "2", secret: false }], false),
    ];
    expect(capturedAfter(steps, 2).map((c) => [c.name, c.value])).toEqual([
      ["token", "<redacted>"],
      ["id", "1"],
    ]);
    expect(capturedAfter(steps, 3).find((c) => c.name === "id")?.value).toBe("2");
    expect(reportCounts([{ number: 1, data: null, passed: false, total: 4, skipped: 1, steps }])).toEqual({
      steps: 3,
      failed: 1,
      skipped: 1,
      checks: 3,
    });
  });
});
