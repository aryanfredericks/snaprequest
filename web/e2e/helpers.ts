import { expect, type Page } from "@playwright/test";

export const MOCK = process.env.SNAPREQ_E2E_MOCK ?? "http://127.0.0.1:8765";
export const WORKSPACE = process.env.SNAPREQ_E2E_WORKSPACE ?? "";

/** Replaces the text of a CodeMirror editor found by its accessible name. */
export async function fill(page: Page, label: string, text: string) {
  const editor = page.getByRole("textbox", { name: label, exact: true });
  await editor.click();
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.press("Delete");
  if (text) await page.keyboard.insertText(text);
}

export async function openShopRequest(page: Page, folder: string | null, name: string) {
  const tree = page.getByRole("tree", { name: "Collections" });
  if (!(await tree.getByRole("button", { name, exact: false }).isVisible().catch(() => false))) {
    await tree.getByRole("button", { name: "Shop API" }).click();
    if (folder) await tree.getByRole("button", { name: folder }).click();
  }
  await tree.getByRole("button", { name }).click();
  await expect(page.getByRole("textbox", { name: "Request name" })).toHaveValue(name);
}

export async function selectEnv(page: Page, env: string) {
  await page.getByLabel("Environment").selectOption(env);
}
