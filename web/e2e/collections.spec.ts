import { existsSync, readFileSync } from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import { fill, WORKSPACE } from "./helpers";

const tree = (page: Page) => page.getByRole("tree", { name: "Collections" });

async function menu(page: Page, item: string, action: string) {
  await tree(page).getByRole("button", { name: `Actions for ${item}`, exact: true }).click();
  await page.getByRole("menuitem", { name: action }).click();
}

async function dialog(page: Page, values: Record<string, string>, submit: string) {
  const d = page.getByRole("dialog");
  for (const [label, value] of Object.entries(values)) {
    const field = d.getByLabel(label, { exact: true });
    if ((await field.evaluate((el) => el.tagName)) === "SELECT") await field.selectOption(value);
    else await field.fill(value);
  }
  await d.getByRole("button", { name: submit }).click();
  await expect(d).toBeHidden();
}

test("collections, folders and requests are created, renamed, moved and deleted", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New collection", exact: true }).click();
  await dialog(page, { "Directory name (id)": "tree-e2e", "Display name": "Tree E2E" }, "Create");
  await tree(page).getByRole("button", { name: "Tree E2E", exact: true }).click();

  await menu(page, "Tree E2E", "New folder…");
  await dialog(page, { "Folder name": "users" }, "Create");
  await menu(page, "Tree E2E", "New request…");
  await dialog(page, { "File name": "health", Method: "GET" }, "Create");
  await expect(page.getByRole("textbox", { name: "URL", exact: true })).toContainText("{{base_url}}/");
  await menu(page, "Tree E2E", "New request…");
  await dialog(page, { "File name": "create", Method: "POST" }, "Create");
  await expect.poll(() => existsSync(`${WORKSPACE}/tree-e2e/create.toml`)).toBe(true);

  // Drag `create` into `users`.
  await tree(page).getByRole("button", { name: "POST create", exact: true }).dragTo(tree(page).getByRole("button", { name: "users", exact: true }));
  await expect.poll(() => existsSync(`${WORKSPACE}/tree-e2e/users/create.toml`)).toBe(true);
  expect(existsSync(`${WORKSPACE}/tree-e2e/create.toml`)).toBe(false);
  // The open tab follows the move.
  await expect(page.getByText("tree-e2e / users/create")).toBeVisible();

  // Rename, then reorder with the keyboard-accessible menu.
  await menu(page, "health", "Rename…");
  await dialog(page, { "New file name": "ping" }, "Rename");
  await expect.poll(() => existsSync(`${WORKSPACE}/tree-e2e/ping.toml`)).toBe(true);
  const rows = async () => {
    const all = (await tree(page).locator("[data-tree-row]").allInnerTexts()).map((t) => t.replace(/\s+/g, " ").trim());
    return all.slice(all.indexOf("▾ Tree E2E") + 1);
  };
  await expect.poll(rows).toEqual(["GET ping", "▾ 📁 users", "POST create"]);
  await menu(page, "ping", "Move down");
  await expect.poll(rows).toEqual(["▾ 📁 users", "POST create", "GET ping"]);
  expect(readFileSync(`${WORKSPACE}/tree-e2e/snapreq.toml`, "utf8")).toContain('order = ["users", "ping"]');

  // Delete the folder with its request.
  await menu(page, "users", "Delete…");
  await page.getByRole("dialog").getByRole("button", { name: "Delete", exact: true }).click();
  await expect.poll(() => existsSync(`${WORKSPACE}/tree-e2e/users`)).toBe(false);
  await expect(tree(page).getByRole("button", { name: "users", exact: true })).toHaveCount(0);
});

test("environments hold variables and secrets, and switching them changes what requests see", async ({ page }) => {
  await page.goto("/");
  await tree(page).getByRole("button", { name: "Shop API", exact: true }).click();
  await tree(page).getByRole("button", { name: "New environment in Shop API", exact: true }).click();
  await dialog(page, { Name: "staging" }, "Create");

  const env = page.getByRole("region", { name: "Environment staging" });
  await env.getByRole("button", { name: "+ Add variable", exact: true }).click();
  await env.getByLabel("Environment variables name 1").fill("base_url");
  await env.getByLabel("Environment variables value 1").fill("http://staging.invalid");
  await env.getByRole("button", { name: "+ Add variable", exact: true }).click();
  await env.getByLabel("Environment variables name 2").fill("api_key");
  await env.getByLabel("api_key is secret").check();
  await env.getByRole("button", { name: "Save", exact: true }).click();
  await expect(env.getByRole("button", { name: "Saved", exact: true })).toBeVisible();
  await expect(env).toContainText("no value yet");
  await env.getByRole("button", { name: "Set value…", exact: true }).click();
  await dialog(page, { Value: "very-secret-key" }, "Store");
  await expect(env).toContainText("value set");

  const file = readFileSync(`${WORKSPACE}/shop/environments/staging.toml`, "utf8");
  expect(file).toContain('base_url = "http://staging.invalid"');
  expect(file).toContain("api_key = { secret = true }");
  expect(file).not.toContain("very-secret-key");
  // Stored sealed (docs/SERVER.md, "Secret values at rest").
  const sealed = readFileSync(`${WORKSPACE}/shop/.secrets/staging.toml`, "utf8");
  expect(sealed).toContain('api_key = "enc:v1:');
  expect(sealed).not.toContain("very-secret-key");

  // The request's URL resolves against the selected environment.
  await tree(page).getByRole("button", { name: "GET Health", exact: true }).click();
  await page.getByRole("combobox", { name: "Environment", exact: true }).selectOption("staging");
  const url = page.getByRole("textbox", { name: "URL", exact: true });
  const tip = async () => {
    await page.mouse.move(0, 0);
    await url.locator(".cm-var-defined").hover();
    return page.locator(".cm-var-tip").innerText();
  };
  await expect.poll(tip).toBe("base_url = http://staging.invalid (environment `staging`)");
  await page.getByRole("combobox", { name: "Environment", exact: true }).selectOption("ci");
  await expect.poll(tip).toBe("base_url = http://127.0.0.1:8765 (environment `ci`)");
});

test("collection settings are edited and saved", async ({ page }) => {
  await page.goto("/");
  await tree(page).getByRole("button", { name: "Shop API", exact: true }).click();
  await menu(page, "Shop API", "Collection settings");
  const settings = page.getByRole("region", { name: "Collection settings" });
  await settings.getByRole("tab", { name: /Headers/ }).click();
  await settings.getByRole("button", { name: "+ Add", exact: true }).click();
  await settings.getByLabel("Headers name 2").fill("X-Client");
  await fill(page, "Headers value 2", "snapreq-e2e");
  await settings.getByRole("button", { name: "Save", exact: true }).click();
  await expect(settings.getByRole("button", { name: "Saved", exact: true })).toBeVisible();
  const file = readFileSync(`${WORKSPACE}/shop/snapreq.toml`, "utf8");
  expect(file).toContain('X-Client = "snapreq-e2e"');
  expect(file).toContain('token = "{{token}}"');
  expect(file).toContain("password = { secret = true }");
});
