import { readFileSync } from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import { BASE, selectEnv, WORKSPACE } from "./helpers";

const tree = (page: Page) => page.getByRole("tree", { name: "Collections" });

async function openShop(page: Page) {
  await page.goto("/");
  await page.getByRole("tab", { name: "Collections" }).click();
  await tree(page).getByRole("button", { name: "Shop API", exact: true }).click();
}

test("a collection runs with live progress, a report and downloads", async ({ page }) => {
  await openShop(page);
  await tree(page).getByRole("button", { name: "Actions for Shop API", exact: true }).click();
  await page.getByRole("menuitem", { name: "Run collection…" }).click();
  const runner = page.getByRole("region", { name: "Runner" });
  await runner.getByLabel("Environment").selectOption("ci");
  await runner.getByRole("button", { name: "Run", exact: true }).click();
  const report = runner.getByRole("region", { name: "Run report" });
  await expect(report.getByRole("status", { name: "Run summary" })).toContainText("Passed");
  await expect(report.getByRole("table", { name: "Steps" })).toContainText("auth/login");

  // A step's response, checks and debug view.
  const show = report.getByRole("button", { name: /Show step \d+ orders\/list/ });
  const number = (await show.getAttribute("aria-label"))!.match(/step (\d+)/)![1];
  await show.click();
  const step = report.getByRole("region", { name: `Step ${number} response` });
  await expect(step.getByRole("status")).toContainText("200");
  await step.getByRole("tab", { name: "Debug", exact: true }).click();
  await expect(step.getByRole("table", { name: "Timing waterfall" })).toBeVisible();
  await step.getByRole("tab", { name: "Wire log" }).click();
  await expect(step.getByRole("textbox", { name: "Bytes sent" })).toContainText("GET /orders HTTP/1.1");
  await expect(step.getByRole("textbox", { name: "Bytes sent" })).not.toContainText("demo-token");

  // Reports download as files.
  for (const [name, needle] of [["HTML", "<!doctype html"], ["JUnit", "<testsuites"], ["JSON", '"format": 1']] as const) {
    const href = await report.getByRole("link", { name, exact: true }).getAttribute("href");
    const res = await page.request.get(`${BASE}${href}`);
    expect(res.ok()).toBe(true);
    expect((await res.text()).toLowerCase()).toContain(needle.toLowerCase());
    expect(res.headers()["content-disposition"]).toMatch(/^attachment/);
  }

  // The run is listed in the Runs panel and opens again.
  await page.getByRole("tab", { name: "Runs" }).click();
  await page.getByRole("list", { name: "Runs" }).getByRole("button").first().click();
  await expect(page.getByRole("status", { name: "Run summary" }).last()).toContainText("Passed");
});

test("data rows run one iteration each", async ({ page }) => {
  await openShop(page);
  await tree(page).getByRole("button", { name: "Orders", exact: true }).click();
  await tree(page).getByRole("button", { name: "Actions for Orders", exact: true }).click();
  await page.getByRole("menuitem", { name: "Run folder…" }).click();
  const runner = page.getByRole("region", { name: "Runner" });
  await runner.getByLabel("Environment").selectOption("ci");
  await runner.getByText(/^Data \(/).click();
  await runner.getByLabel("CSV with a header row, or a JSON array of objects").fill("note\nfirst\nsecond\nthird");
  await expect(runner.getByText("Data (3 rows: one iteration each)")).toBeVisible();
  await runner.getByRole("button", { name: "Run", exact: true }).click();
  const report = runner.getByRole("region", { name: "Run report" });
  // Without logging in, the orders requests fail: every iteration is reported.
  await expect(report.getByRole("status", { name: "Run summary" })).toContainText("0 of 3 iterations passed");
  await expect(report.getByRole("table", { name: "Iteration 3" })).toBeVisible();
  await expect(report).toContainText("note=second");
});

test("a flow runs step by step, shows captured values and resumes from a step", async ({ page }) => {
  await openShop(page);
  await tree(page).getByRole("button", { name: "Checkout", exact: true }).click();
  await selectEnv(page, "ci");
  const flow = page.getByRole("region", { name: "Flow orders/checkout" });
  const steps = flow.getByRole("list", { name: "Steps" });

  await flow.getByRole("button", { name: "Step", exact: true }).click();
  await expect(steps.getByRole("button", { name: "Step 1: auth/login, passed" })).toBeVisible();
  await expect(flow.getByRole("table", { name: "Captured values after step 1" })).toContainText("token");
  await expect(flow.getByRole("table", { name: "Captured values after step 1" })).toContainText("<redacted>");
  await flow.getByRole("button", { name: "Step (2)" }).click();
  await expect(steps.getByRole("button", { name: "Step 2: Create order, passed" })).toBeVisible();
  await expect(flow.getByRole("table", { name: "Captured values after step 2" })).toContainText("order_id");

  // The rest, then step 3 again from the state step 2 left.
  await flow.getByRole("button", { name: "Run from step 3" }).click();
  await expect(steps.getByRole("button", { name: "Step 5: Gone, passed" })).toBeVisible();
  await flow.getByRole("button", { name: "Run all" }).click();
  await expect(steps.getByRole("button", { name: /Step 5: Gone, passed/ })).toBeVisible();
  await steps.getByRole("button", { name: /Step 3: Wait until ready/ }).click();
  const detail = flow.getByRole("region", { name: "Step 3 response" });
  await expect(detail.getByRole("status")).toContainText("200");
  await expect(flow.getByRole("table", { name: "Retry until" })).toContainText("$.state");
  await detail.getByRole("tab", { name: "Debug", exact: true }).click();
  await expect(detail.getByRole("table", { name: "Timing waterfall" })).toBeVisible();
  await detail.getByRole("tab", { name: "Wire log" }).click();
  await expect(detail.getByRole("textbox", { name: "Bytes sent" })).toContainText("GET /orders/");
});

test("flow steps are edited, reordered and saved", async ({ page }) => {
  await openShop(page);
  await tree(page).getByRole("button", { name: "Checkout", exact: true }).click();
  const flow = page.getByRole("region", { name: "Flow orders/checkout" });
  const steps = flow.getByRole("list", { name: "Steps" });
  await steps.getByRole("button", { name: /Step 4: Cancel/ }).click();
  await flow.getByRole("textbox", { name: "Step 4 name" }).fill("Cancel the order");
  await flow.getByRole("button", { name: "Move step 4 up" }).click();
  await expect(steps.getByRole("button", { name: /^Step 3: Cancel the order/ })).toBeVisible();
  await flow.getByRole("button", { name: "+ Inline request" }).click();
  await expect(steps.getByRole("listitem")).toHaveCount(6);
  await flow.getByRole("button", { name: "Remove step 6" }).click();
  await flow.getByRole("button", { name: "Save*" }).click();
  await expect(flow.getByRole("button", { name: "Saved" })).toBeVisible();
  const file = readFileSync(`${WORKSPACE}/shop/orders/checkout.flow.toml`, "utf8");
  expect(file.indexOf('name = "Cancel the order"')).toBeLessThan(file.indexOf('name = "Wait until ready"'));
  expect(file).toContain('until.json');
});
