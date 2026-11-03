import { expect, test, type Page } from "@playwright/test";
import { BASE, fill, MOCK } from "./helpers";

async function send(page: Page, url: string) {
  await page.goto("/");
  await page.getByRole("button", { name: "New request" }).first().click();
  await fill(page, "URL", url);
  await page.keyboard.press("ControlOrMeta+Enter");
  const response = page.getByRole("region", { name: "Response" });
  await expect(response.getByRole("status").filter({ hasText: /^\d{3}/ }).or(response.getByRole("alert"))).toBeVisible();
}

test("JSON is pretty-printed, raw stays as sent, and the body can be searched", async ({ page }) => {
  await send(page, `${MOCK}/catalog?items=3`);
  const response = page.getByRole("region", { name: "Response" });
  await expect(response.getByRole("status")).toContainText("200 OK");
  const body = response.getByRole("textbox", { name: "Response body" });
  await expect(body).toContainText('"sku": "sku-1"');
  await response.getByRole("radio", { name: "Raw" }).click();
  await expect(body).toContainText('[{"sku":"sku-1"');
  await response.getByRole("button", { name: "Search" }).click();
  await page.keyboard.insertText("sku-2");
  await page.keyboard.press("Enter");
  await expect(response.locator(".cm-searchMatch").first()).toBeVisible();
});

test("headers and cookies are listed", async ({ page }) => {
  await send(page, `${MOCK}/catalog`);
  const response = page.getByRole("region", { name: "Response" });
  await response.getByRole("tab", { name: /Headers/ }).click();
  await expect(response.getByRole("table", { name: "Response headers" })).toContainText("application/json");
  await response.getByRole("tab", { name: /Cookies/ }).click();
  const cookies = response.getByRole("table", { name: "Cookies set by the response" });
  await expect(cookies).toContainText("visited");
  await expect(cookies).toContainText("HttpOnly");
  await expect(cookies).toContainText("in 600 s");
});

test("HTML is previewed in a sandboxed frame", async ({ page }) => {
  await send(page, `${MOCK}/page`);
  const response = page.getByRole("region", { name: "Response" });
  await response.getByRole("radio", { name: "Preview" }).click();
  const frame = response.locator("iframe[title='HTML preview']");
  await expect(frame).toHaveAttribute("sandbox", "");
  await expect(page.frameLocator("iframe[title='HTML preview']").getByRole("heading")).toHaveText("Welcome to the shop");
});

test("large bodies stay responsive and render only what is visible", async ({ page }) => {
  await send(page, `${MOCK}/catalog?items=40000`);
  const response = page.getByRole("region", { name: "Response" });
  await expect(response.getByRole("status")).toContainText("200 OK");
  await expect(response).toContainText("MiB");
  // CodeMirror renders the lines in view, not the ~280 000 lines of the pretty body.
  const lines = await response.locator(".cm-line").count();
  expect(lines).toBeLessThan(2000);
  await response.getByRole("radio", { name: "Hex" }).click();
  const rows = await response.getByRole("list", { name: "Hex dump" }).getByRole("listitem").count();
  expect(rows).toBeLessThan(200);
});

test("images are previewed", async ({ page }) => {
  await send(page, `${BASE}/favicon.svg`);
  const response = page.getByRole("region", { name: "Response" });
  await response.getByRole("radio", { name: "Preview" }).click();
  await expect(response.getByRole("img", { name: "Response body" })).toBeVisible();
});

test("connection failures name the phase", async ({ page }) => {
  await send(page, "http://127.0.0.1:1/");
  await expect(page.getByRole("region", { name: "Response" }).getByRole("alert")).toContainText("Failed during TCP connect");
});
