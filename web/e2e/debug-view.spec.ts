import { expect, test, type Page } from "@playwright/test";
import { fill, MOCK } from "./helpers";

async function sendAndDebug(page: Page, url: string) {
  await page.goto("/");
  await page.getByRole("button", { name: "New request" }).first().click();
  await fill(page, "URL", url);
  await page.keyboard.press("ControlOrMeta+Enter");
  const response = page.getByRole("region", { name: "Response" });
  await expect(response.getByRole("tab", { name: "Debug", exact: true })).toBeVisible();
  await response.getByRole("tab", { name: "Debug", exact: true }).click();
  return response;
}

test("the waterfall shows every phase of every redirect hop", async ({ page }) => {
  const response = await sendAndDebug(page, `${MOCK}/old-health`);
  await expect(response.getByRole("status")).toContainText("200 OK");
  const waterfall = response.getByRole("table", { name: "Timing waterfall" });
  await expect(waterfall).toContainText(`hop 1/2: 302 → ${MOCK}/health`);
  await expect(waterfall).toContainText("hop 2/2: final response");
  for (const phase of ["send", "wait", "download"]) {
    await expect(waterfall.getByRole("rowheader", { name: phase, exact: true })).toHaveCount(2);
  }
  // The second hop reuses the connection.
  await expect(waterfall).toContainText("connection reused");

  await response.getByRole("tab", { name: "Redirects & connection" }).click();
  const hops = response.getByRole("list", { name: "Hops" });
  await expect(hops.getByRole("listitem")).toHaveCount(2);
  await expect(hops.getByRole("listitem").first()).toContainText("302");
  await expect(hops.getByRole("listitem").last()).toContainText("(reused)");
});

test("the wire log shows the raw bytes of each hop", async ({ page }) => {
  const response = await sendAndDebug(page, `${MOCK}/old-health`);
  await response.getByRole("tab", { name: "Wire log" }).click();
  await expect(response.getByRole("textbox", { name: "Bytes sent" })).toContainText("GET /health HTTP/1.1");
  await expect(response.getByRole("textbox", { name: "Bytes received" })).toContainText("HTTP/1.1 200 OK");
  await response.getByLabel("Hop").selectOption({ index: 1 });
  await expect(response.getByRole("textbox", { name: "Bytes sent" })).toContainText("GET /old-health HTTP/1.1");
  await expect(response.getByRole("textbox", { name: "Bytes received" })).toContainText("302 Found");
});

test("a new connection shows DNS and connect; the event list explains each step", async ({ page }) => {
  // `localhost` is used nowhere else, so the server's pool has no connection to reuse.
  const url = MOCK.replace("127.0.0.1", "localhost");
  const response = await sendAndDebug(page, `${url}/health`);
  const waterfall = response.getByRole("table", { name: "Timing waterfall" });
  for (const phase of ["DNS", "connect", "send", "wait", "download"]) {
    await expect(waterfall.getByRole("rowheader", { name: phase, exact: true })).toBeVisible();
  }
  await response.getByRole("tab", { name: /Events/ }).click();
  const events = response.getByRole("list", { name: "Trace events" });
  await expect(events).toContainText("resolved localhost");
  await expect(events).toContainText(/connected to .*:8765 from/);
  await expect(events).toContainText(`GET ${url}/health HTTP/1.1`);
  await events.getByRole("button", { name: "headers" }).first().click();
  await expect(response.getByRole("table", { name: "Request headers sent" })).toContainText(/user-agent/i);
});

test("a failed request still shows its trace", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New request" }).first().click();
  await fill(page, "URL", "http://127.0.0.1:1/");
  await page.keyboard.press("ControlOrMeta+Enter");
  const response = page.getByRole("region", { name: "Response" });
  await expect(response.getByRole("alert")).toContainText("TCP connect");
  await expect(response.getByRole("tab", { name: "Debug", exact: true })).toHaveAttribute("aria-selected", "true");
  await response.getByRole("tab", { name: /Events/ }).click();
  await expect(response.getByRole("list", { name: "Trace events" })).toContainText("TCP connect:");
});
