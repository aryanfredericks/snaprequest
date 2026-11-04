import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { fill, MOCK } from "./helpers";

// A second server, started by scripts/web-e2e.sh with `--auth accounts` and users ana and bo.
const ACCOUNTS = process.env.SNAPREQ_E2E_ACCOUNTS_URL ?? "http://127.0.0.1:7882";

async function login(page: Page, user: string, password = `${user}-password`) {
  await page.getByLabel("User name").fill(user);
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByRole("button", { name: "Log in" }).click();
}

test("users log in, see only their own history, manage tokens and log out", async ({ page }) => {
  await page.goto(`${ACCOUNTS}/`);
  await expect(page.getByRole("heading", { name: "SnapRequest" })).toBeVisible();
  const axe = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"]).analyze();
  expect(axe.violations).toEqual([]);

  await login(page, "ana", "not-her-password");
  await expect(page.getByRole("alert")).toContainText("wrong user name or password");
  await login(page, "ana");
  const account = page.getByRole("button", { name: /ana$/ });
  await expect(account).toBeVisible();

  // Ana sends a request; it is in her history.
  await page.getByRole("button", { name: "New request" }).first().click();
  await fill(page, "URL", `${MOCK}/health?tag=ana-only`);
  await page.getByRole("button", { name: "Send" }).click();
  await expect(page.getByRole("region", { name: "Response" })).toContainText("200");
  await page.getByRole("tab", { name: "History" }).click();
  await expect(page.getByRole("list", { name: "History" })).toContainText("tag=ana-only");

  // An API token is shown once, then listed by its last characters, and can be revoked.
  await account.click();
  const dialog = page.getByRole("dialog", { name: /Account: ana/ });
  await dialog.getByLabel("Name").fill("ci");
  await dialog.getByRole("button", { name: "Create token" }).click();
  const secret = await dialog.getByLabel("New token").inputValue();
  expect(secret).toMatch(/^snapreq_pat_/);
  const resp = await page.request.get(`${ACCOUNTS}/api/v1/history`, {
    headers: { Authorization: `Bearer ${secret}` },
  });
  expect((await resp.json()).entries[0].url).toContain("tag=ana-only");
  await expect(dialog.getByRole("cell", { name: `…${secret.slice(-4)}` })).toBeVisible();
  await dialog.getByRole("button", { name: "Revoke ci" }).click();
  await expect(dialog.getByText("No tokens.")).toBeVisible();

  await dialog.getByRole("button", { name: "Log out" }).click();
  await expect(page.getByRole("button", { name: "Log in" })).toBeVisible();

  // Bo sees none of it: he is in no workspace until he creates one.
  await login(page, "bo");
  await expect(page.getByRole("heading", { name: "No workspace yet" })).toBeVisible();
  await page.getByLabel("Id (directory)").fill("bo-space");
  await page.getByLabel("Name").fill("Bo's space");
  await page.getByRole("button", { name: "Create workspace" }).click();
  await expect(page.getByRole("combobox", { name: "Workspace" })).toHaveValue("bo-space");
  await expect(page.getByRole("button", { name: /bo$/ })).toBeVisible();
  await page.getByRole("tab", { name: "History" }).click();
  await expect(page.getByText("Requests you send appear here.")).toBeVisible();
  await expect(page.getByText("tag=ana-only")).toHaveCount(0);
  await expect(page.getByRole("combobox", { name: "Workspace" }).getByRole("option")).toHaveText(["Bo's space"]);
});

test("owners share a workspace; viewers cannot change it", async ({ page }) => {
  await page.goto(`${ACCOUNTS}/`);
  await login(page, "ana");
  await page.getByRole("button", { name: "Workspace settings" }).click();
  const dialog = page.getByRole("dialog", { name: /Workspace: Default/ });
  await dialog.getByLabel("User name").fill("bo");
  await dialog.getByRole("combobox", { name: "Role of the new member" }).selectOption("viewer");
  await dialog.getByRole("button", { name: "Add member" }).click();
  await expect(dialog.getByRole("combobox", { name: "Role of bo" })).toHaveValue("viewer");
  await dialog.getByRole("button", { name: "Close" }).click();
  await page.getByRole("button", { name: /ana$/ }).click();
  await page.getByRole("dialog", { name: /Account: ana/ }).getByRole("button", { name: "Log out" }).click();

  await login(page, "bo");
  const picker = page.getByRole("combobox", { name: "Workspace" });
  await expect(picker.getByRole("option")).toHaveText(["Default", "Bo's space"]);
  await picker.selectOption("default");
  await expect(page.getByText("read-only")).toBeVisible();
  // Changing anything is refused.
  await page.getByRole("button", { name: "New collection" }).click();
  const ask = page.getByRole("dialog", { name: "New collection" });
  await ask.getByRole("textbox").first().fill("bo-was-here");
  await ask.getByRole("button", { name: "Create" }).click();
  await expect(page.getByText(/a viewer of workspace `default`/)).toBeVisible();
});

test("an ended session brings back the login", async ({ page, context }) => {
  await page.goto(`${ACCOUNTS}/`);
  await login(page, "ana");
  await expect(page.getByRole("button", { name: /ana$/ })).toBeVisible();
  await context.clearCookies();
  await page.getByRole("tab", { name: "History" }).click();
  await page.getByRole("tab", { name: "Runs" }).click();
  await expect(page.getByText("Your session has ended")).toBeVisible();
  await login(page, "ana");
  await expect(page.getByRole("button", { name: /ana$/ })).toBeVisible();
});
