import { expect, test, type Page } from "@playwright/test";
import { fill, MOCK } from "./helpers";

async function send(page: Page, url: string, method = "GET") {
  await page.getByRole("button", { name: "New request" }).first().click();
  if (method !== "GET") await page.getByLabel("Method").selectOption(method);
  await fill(page, "URL", url);
  await page.keyboard.press("ControlOrMeta+Enter");
  const response = page.getByRole("region", { name: "Response" });
  await expect(response.getByRole("status").filter({ hasText: /^\d{3}/ }).or(response.getByRole("alert"))).toBeVisible();
}

const history = (page: Page) => page.getByRole("list", { name: "History" });

test("history lists sent requests and filters them", async ({ page }) => {
  await page.goto("/");
  await send(page, `${MOCK}/catalog?items=2&tag=hist-a`);
  await send(page, `${MOCK}/nope?tag=hist-a`);
  await send(page, `${MOCK}/health?tag=hist-b`, "POST");
  await page.getByRole("tab", { name: "History" }).click();
  await page.getByLabel("Search history by URL").fill("tag=hist-");
  await expect(history(page).getByRole("listitem")).toHaveCount(3);
  await page.getByLabel("Status filter").selectOption("4");
  await expect(history(page).getByRole("listitem")).toHaveCount(2);
  await page.getByLabel("Method filter").selectOption("POST");
  await expect(history(page).getByRole("listitem")).toHaveCount(1);
  await expect(history(page)).toContainText("/health?tag=hist-b");
  await page.getByLabel("Status filter").selectOption("");
  await page.getByLabel("Method filter").selectOption("");
  await page.getByLabel("Search history by URL").fill("tag=hist-a");
  await expect(history(page).getByRole("listitem")).toHaveCount(2);
});

test("a history entry shows the stored exchange and re-runs it", async ({ page }) => {
  await page.goto("/");
  await send(page, `${MOCK}/catalog?items=1&tag=rerun`);
  await page.getByRole("tab", { name: "History" }).click();
  await page.getByLabel("Search history by URL").fill("tag=rerun");
  await history(page).getByRole("button", { name: /catalog\?items=1&tag=rerun/ }).click();
  const stored = page.getByRole("region", { name: "Stored response" });
  await expect(stored.getByRole("status")).toContainText("200 OK");
  await expect(stored.getByRole("textbox", { name: "Response body" })).toContainText('"sku": "sku-1"');
  await stored.getByRole("tab", { name: "Debug", exact: true }).click();
  await expect(stored.getByRole("table", { name: "Timing waterfall" })).toBeVisible();

  await page.getByRole("button", { name: "Re-run" }).click();
  await expect(page.getByRole("region", { name: "Response" }).getByRole("status")).toContainText("200 OK");
  await expect(history(page).getByRole("listitem")).toHaveCount(2);
});

test("credentials are not re-sent from history", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New request" }).first().click();
  await fill(page, "URL", `${MOCK}/orders?tag=cred`);
  await page.getByRole("tab", { name: /Headers/ }).click();
  await page.getByRole("button", { name: "+ Add" }).click();
  await page.getByLabel("Headers name 1").fill("Authorization");
  await fill(page, "Headers value 1", "Bearer demo-token-7f3a9c");
  await page.keyboard.press("ControlOrMeta+Enter");
  await expect(page.getByRole("region", { name: "Response" }).getByRole("status")).toContainText("200 OK");
  await page.getByRole("tab", { name: "History" }).click();
  await page.getByLabel("Search history by URL").fill("tag=cred");
  await history(page).getByRole("button", { name: /orders\?tag=cred/ }).click();
  await page.getByText(/Request headers/).click();
  await expect(page.getByRole("table", { name: "Stored request headers" })).toContainText("<redacted>");
  await expect(page.getByRole("table", { name: "Stored request headers" })).not.toContainText("demo-token");
  await page.getByRole("button", { name: "Re-run" }).click();
  await expect(page.getByRole("alert").filter({ hasText: "Credentials are not kept" })).toContainText("Authorization");
});

test("two responses are compared", async ({ page }) => {
  await page.goto("/");
  await send(page, `${MOCK}/catalog?items=2&tag=cmp`);
  await send(page, `${MOCK}/catalog?items=3&tag=cmp`);
  await page.getByRole("tab", { name: "History" }).click();
  await page.getByLabel("Search history by URL").fill("tag=cmp");
  const boxes = history(page).getByRole("checkbox");
  await boxes.nth(0).check();
  await boxes.nth(1).check();
  await page.getByRole("button", { name: /Compare/ }).click();
  const diff = page.getByRole("region", { name: "Compare responses" });
  await expect(diff.locator(".cm-mergeView")).toBeVisible();
  await expect(diff.locator(".cm-changedLine").first()).toBeVisible();
  await expect(diff).toContainText("sku-3");
  await diff.getByRole("tab", { name: "Timing" }).click();
  await expect(diff.getByRole("table", { name: "Timing comparison" })).toContainText("total");
});
