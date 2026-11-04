import { expect, test } from "@playwright/test";
import { fill, openShopRequest, selectEnv } from "./helpers";

test("a test script written in the editor runs when the request is sent", async ({ page }) => {
  await page.goto("/");
  await openShopRequest(page, null, "Health");
  await selectEnv(page, "ci");
  await page.getByRole("tab", { name: /Scripts/ }).click();
  await fill(page, "Pre-request script", 'snapreq.request.headers.set("X-From-Script", "yes"); console.log("about to send");');
  await fill(
    page,
    "Test script",
    'snapreq.test("health is 200", () => snapreq.expect(snapreq.response.status).toBe(200));\nsnapreq.test("wrong on purpose", () => snapreq.expect(1).toBe(2));',
  );
  await page.keyboard.press("ControlOrMeta+Enter");
  const response = page.getByRole("region", { name: "Response" });
  await response.getByRole("tab", { name: /Checks/ }).click();
  const checks = response.getByRole("table", { name: "Checks" });
  await expect(checks).toContainText("health is 200");
  await expect(checks).toContainText("expected 1 to be 2");
  await expect(response.getByRole("region", { name: "Script output" })).toContainText("about to send");
  // Unsaved: the request file is unchanged until saved.
  await expect(page.getByRole("button", { name: "Save*" })).toBeVisible();
});
