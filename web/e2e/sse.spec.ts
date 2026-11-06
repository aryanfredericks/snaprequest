import { expect, test } from "@playwright/test";
import { fill, MOCK } from "./helpers";

test("a Server-Sent Events stream shows its events live and can be stopped", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New request" }).first().click();
  await fill(page, "URL", `${MOCK}/events?n=0`);
  await page.getByRole("button", { name: "Send" }).click();
  const response = page.getByRole("region", { name: "Response" });
  await expect(response.getByRole("tab", { name: /Events \(live\)/ })).toBeVisible();
  const list = response.getByRole("list", { name: "Events" });
  await expect(list.getByRole("listitem").nth(2)).toContainText('{"tick": 2}');
  await page.getByRole("button", { name: "Stop" }).click();
  // The response ends with what arrived, and opens on its events.
  await expect(response).toContainText("200");
  await expect(response.getByRole("tab", { name: /^Events/ })).toHaveAttribute("aria-selected", "true");
  const count = await list.getByRole("listitem").count();
  expect(count).toBeGreaterThanOrEqual(3);
  await response.getByRole("combobox", { name: "Event type" }).selectOption("tick");
  await expect(list.getByRole("listitem").first()).toContainText("tick");
  await list.getByRole("button", { name: "pretty" }).first().click();
  await expect(list.locator("pre").first()).toContainText('"tick": 0');
  await expect(page.getByRole("button", { name: "Send" })).toBeVisible();
});
