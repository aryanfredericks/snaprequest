import { readFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import { fill, MOCK, openShopRequest, selectEnv, WORKSPACE } from "./helpers";

test("a scratch request is sent and shows its status", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New request" }).first().click();
  await fill(page, "URL", `${MOCK}/health`);
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page.getByRole("region", { name: "Response" })).toContainText("200");
});

test("collection requests use the environment and highlight variables", async ({ page }) => {
  await page.goto("/");
  await openShopRequest(page, null, "Health");
  const url = page.getByRole("textbox", { name: "URL", exact: true });
  await selectEnv(page, "ci");
  await expect(url.locator(".cm-var-defined")).toHaveText("{{base_url}}");
  await page.keyboard.press("ControlOrMeta+Enter");
  await expect(page.getByRole("region", { name: "Response" })).toContainText("200");

  // An undefined variable is marked.
  await fill(page, "URL", "{{base_url}}/health?x={{nope}}");
  await expect(url.locator(".cm-var-undefined")).toHaveText("{{nope}}");
});

test("edits are sent unsaved and saved with Ctrl+S", async ({ page }) => {
  await page.goto("/");
  await openShopRequest(page, "Orders", "List orders");
  await page.getByRole("tab", { name: /Headers/ }).click();
  await page.getByRole("button", { name: "+ Add" }).click();
  await page.getByLabel("Headers name 1").fill("X-Trace");
  await fill(page, "Headers value 1", "e2e-{{user}}");
  await expect(page.getByRole("button", { name: "Save*" })).toBeVisible();
  await page.keyboard.press("ControlOrMeta+s");
  await expect(page.getByRole("button", { name: "Save", exact: true })).toBeVisible();
  const file = readFileSync(`${WORKSPACE}/shop/orders/list.toml`, "utf8");
  expect(file).toContain('X-Trace = "e2e-{{user}}"');
});

test("a scratch request can be saved into a collection", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New request" }).first().click();
  await page.getByLabel("Method").fill("POST");
  await fill(page, "URL", `${MOCK}/login`);
  await page.getByRole("tab", { name: /Body/ }).click();
  await page.getByLabel("JSON").check();
  await fill(page, "Request body", '{"user":"demo","password":"letmein"}');
  await page.getByRole("button", { name: "Save" }).click();
  const dialog = page.getByRole("dialog", { name: "Save request" });
  await dialog.getByLabel(/Key/).fill("auth/login-copy");
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(page.getByText("shop / auth/login-copy")).toBeVisible();
  const file = readFileSync(`${WORKSPACE}/shop/auth/login-copy.toml`, "utf8");
  expect(file).toContain('method = "POST"');
  expect(file).toContain("letmein");
  await page.keyboard.press("ControlOrMeta+Enter");
  await expect(page.getByRole("region", { name: "Response" })).toContainText("200");
});

test("Ctrl+Enter in an editor sends without changing the text", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New request" }).first().click();
  await page.getByLabel("Method").fill("POST");
  await fill(page, "URL", `${MOCK}/health`);
  await page.getByRole("tab", { name: /Body/ }).click();
  await page.getByLabel("JSON").check();
  await fill(page, "Request body", '{"a":1}');
  await page.keyboard.press("ControlOrMeta+Enter");
  await expect(page.getByRole("region", { name: "Response" }).getByRole("status")).toContainText("404");
  await expect(page.getByRole("textbox", { name: "Request body" })).toHaveText('{"a":1}');
  await page.getByRole("textbox", { name: "URL", exact: true }).click();
  await page.keyboard.press("ControlOrMeta+Enter");
  await expect(page.getByRole("textbox", { name: "URL", exact: true })).toHaveText(`${MOCK}/health`);
});
