import { expect, type Page } from "@playwright/test";

export const MOCK = process.env.SNAPREQ_E2E_MOCK ?? "http://127.0.0.1:8765";
export const GRPC = process.env.SNAPREQ_E2E_GRPC ?? "127.0.0.1:8766";
export const BASE = process.env.SNAPREQ_E2E_URL ?? "http://127.0.0.1:7881";
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
  const row = tree.getByRole("button", { name: new RegExp(`^[A-Z]+ ${name}$`) });
  if (!(await row.isVisible().catch(() => false))) {
    await tree.getByRole("button", { name: "Shop API", exact: true }).click();
    if (folder) await tree.getByRole("button", { name: folder, exact: true }).click();
  }
  await row.click();
  await expect(page.getByRole("textbox", { name: "Request name" })).toHaveValue(name);
}

export async function selectEnv(page: Page, env: string) {
  await page.getByRole("combobox", { name: "Environment", exact: true }).selectOption(env);
}
