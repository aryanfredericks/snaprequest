import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { fill, MOCK, selectEnv } from "./helpers";

/** No WCAG 2.1 A/AA violations on the page as it is now. */
async function audit(page: Page, what: string) {
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"]).analyze();
  const problems = results.violations.map(
    (v) => `${v.id} (${v.impact}): ${v.help}\n    ${v.nodes.slice(0, 4).map((n) => `${n.target.join(" ")} ${n.failureSummary?.split("\n")[1] ?? ""}`).join("\n    ")}`,
  );
  expect(problems, `${what}:\n${problems.join("\n")}`).toEqual([]);
}

const tree = (page: Page) => page.getByRole("tree", { name: "Collections" });

for (const scheme of ["light", "dark"] as const) {
  test(`every screen meets WCAG 2.1 AA (${scheme})`, async ({ page }) => {
    test.setTimeout(120_000);
    await page.emulateMedia({ colorScheme: scheme });
    await page.goto("/");
    await expect(page.locator("html")).toHaveAttribute("data-theme", scheme);
    await audit(page, "start");

    // Request, response, debug.
    await page.getByRole("button", { name: "New request" }).first().click();
    await fill(page, "URL", `${MOCK}/old-health`);
    await page.keyboard.press("ControlOrMeta+Enter");
    const response = page.getByRole("region", { name: "Response" });
    await expect(response.getByRole("status")).toContainText("200");
    await audit(page, "request and response");
    for (const part of ["Headers", "Cookies", "Debug"]) {
      await response.getByRole("tab", { name: new RegExp(`^${part}`) }).click();
      await audit(page, `response ${part}`);
    }
    for (const part of ["Events", "Redirects & connection", "Wire log"]) {
      await response.getByRole("tab", { name: new RegExp(`^${part}`) }).click();
      await audit(page, `debug ${part}`);
    }
    for (const part of ["Headers", "Body", "Auth", "Settings"]) {
      await page.getByRole("region", { name: "Request" }).getByRole("tab", { name: new RegExp(`^${part}`) }).click();
      await audit(page, `request ${part}`);
    }
    // A second exchange to compare later.
    await page.keyboard.press("ControlOrMeta+Enter");
    await expect(response.getByRole("status")).toContainText("200");

    // Collections, settings, environments, flows, runner.
    await tree(page).getByRole("button", { name: "Shop API", exact: true }).click();
    await tree(page).getByRole("button", { name: "Orders", exact: true }).click();
    await tree(page).getByRole("button", { name: "Actions for Shop API", exact: true }).click();
    await audit(page, "collection menu");
    await page.getByRole("menuitem", { name: "Collection settings" }).click();
    await audit(page, "collection settings");
    await tree(page).getByRole("button", { name: "ci", exact: true }).click();
    await audit(page, "environment");
    await tree(page).getByRole("button", { name: "Checkout", exact: true }).click();
    await selectEnv(page, "ci");
    await audit(page, "flow editor");
    await page.getByRole("button", { name: "Run all" }).click();
    await expect(page.getByRole("list", { name: "Steps" }).getByRole("button", { name: /^5\. .* \(passed\)/ })).toBeVisible();
    await audit(page, "flow results");
    await tree(page).getByRole("button", { name: "Actions for Shop API", exact: true }).click();
    await page.getByRole("menuitem", { name: "Run collection…" }).click();
    await page.getByRole("region", { name: "Runner" }).getByRole("button", { name: "Run", exact: true }).click();
    await expect(page.getByRole("status", { name: "Run summary" })).toBeVisible();
    await audit(page, "run report");

    // History and comparing.
    await page.getByRole("tab", { name: "History" }).click();
    await audit(page, "history");
    const boxes = page.getByRole("list", { name: "History" }).getByRole("checkbox");
    await boxes.nth(0).check();
    await boxes.nth(1).check();
    await page.getByRole("button", { name: /Compare/ }).click();
    await audit(page, "compare");
    await page.getByRole("tab", { name: "Runs" }).click();
    await audit(page, "runs");

    // Dialogs.
    await page.getByRole("tab", { name: "Collections" }).click();
    await page.getByRole("button", { name: "New collection" }).click();
    await audit(page, "prompt dialog");
    await page.getByRole("dialog").getByRole("button", { name: "Cancel" }).click();
    await page.keyboard.press("?");
    await expect(page.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeVisible();
    await audit(page, "shortcuts dialog");
  });
}

test("the theme can be chosen and is remembered", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "light" });
  await page.goto("/");
  await page.getByLabel("Theme").selectOption("dark");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.getByLabel("Theme").selectOption("system");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
});

test("keyboard shortcuts switch tabs and panels", async ({ page }) => {
  await page.goto("/");
  await page.keyboard.press("Alt+n");
  await page.keyboard.press("Alt+n");
  const tabs = page.getByRole("navigation", { name: "Open tabs" }).getByRole("listitem");
  await expect(tabs).toHaveCount(2);
  const active = () => tabs.evaluateAll((items) => items.findIndex((li) => li.querySelector("[aria-current=page]")));
  await expect.poll(active).toBe(1);
  await page.keyboard.press("Alt+BracketLeft");
  await expect.poll(active).toBe(0);
  await page.keyboard.press("Alt+BracketRight");
  await expect.poll(active).toBe(1);
  await page.keyboard.press("Alt+w");
  await expect(tabs).toHaveCount(1);
  await page.keyboard.press("Alt+2");
  await expect(page.getByRole("tab", { name: "History" })).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Alt+1");
  await page.keyboard.press("Alt+l");
  await expect(page.getByRole("textbox", { name: "URL", exact: true })).toBeFocused();
});
