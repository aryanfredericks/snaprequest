import { expect, test } from "@playwright/test";
import { fill, MOCK, openShopRequest, selectEnv } from "./helpers";

test("a request is shown as code without its secret values", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/");
  await openShopRequest(page, "auth", "Log in");
  await selectEnv(page, "ci");
  await page.getByRole("button", { name: "Code", exact: true }).click();
  const d = page.getByRole("dialog", { name: "Request as code" });
  const code = d.getByRole("textbox", { name: "Code" });
  await expect(code).toContainText(`curl`);
  await expect(code).toContainText(`${MOCK}/login`);
  await expect(code).toContainText("${SNAPREQ_SECRET_PASSWORD}");
  await expect(code).not.toContainText("letmein");

  await d.getByLabel("Format").selectOption("python");
  await expect(code).toContainText("import requests");
  await expect(code).toContainText('os.environ["SNAPREQ_SECRET_PASSWORD"]');
  await d.getByRole("button", { name: "Copy" }).click();
  await expect(d.getByRole("status")).toHaveText("Copied to the clipboard.");
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain("requests.request(");
  await d.getByRole("button", { name: "Close" }).click();

  // The choice is remembered; a scratch request works too.
  await page.getByRole("button", { name: "New request" }).first().click();
  await fill(page, "URL", "https://a.test/x");
  await page.getByRole("button", { name: "Code", exact: true }).click();
  await expect(code).toContainText('"https://a.test/x"');
  await expect(code).toContainText("requests.request(");
  await d.getByLabel("Format").selectOption("curl");
  await d.getByRole("button", { name: "Close" }).click();
});

test("a history entry downloads as HAR", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New request" }).first().click();
  await fill(page, "URL", `${MOCK}/catalog?items=1&tag=har-e2e`);
  await page.keyboard.press("ControlOrMeta+Enter");
  await expect(page.getByRole("region", { name: "Response" })).toContainText("200");
  await page.getByRole("tab", { name: "History" }).click();
  await page.getByLabel("Search history by URL").fill("tag=har-e2e");
  await page.getByRole("list", { name: "History" }).getByRole("button", { name: /tag=har-e2e/ }).click();
  const link = page.getByRole("link", { name: "Download HAR" });
  const download = page.waitForEvent("download");
  await link.click();
  const file = await download;
  expect(file.suggestedFilename()).toMatch(/^snapreq-\d+\.har$/);
  const har = JSON.parse(await (await file.createReadStream()).toArray().then((c) => Buffer.concat(c).toString()));
  expect(har.log.entries[0].request.url).toContain("tag=har-e2e");
  expect(har.log.entries[0].response.status).toBe(200);
});
