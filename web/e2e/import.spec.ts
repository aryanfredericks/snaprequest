import { existsSync, readFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import { WORKSPACE } from "./helpers";

test("a pasted cURL command and a Hurl file are imported as collections", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const d = page.getByRole("dialog", { name: "Import" });
  await d.getByLabel("…or paste").fill("curl https://api.example.com/v1/users -H 'Authorization: Bearer e2e-secret-token'");
  await d.getByLabel("Id (directory)").fill("pasted-e2e");
  await d.getByRole("button", { name: "Import", exact: true }).click();
  await expect(d).toBeHidden();
  await expect(page.getByRole("tree", { name: "Collections" }).getByRole("button", { name: "api.example.com", exact: true })).toBeVisible();
  await expect.poll(() => existsSync(`${WORKSPACE}/pasted-e2e/get-v1-users.toml`)).toBe(true);
  const file = readFileSync(`${WORKSPACE}/pasted-e2e/get-v1-users.toml`, "utf8");
  expect(file).toContain("{{token}}");
  expect(file).not.toContain("e2e-secret-token");

  // A file whose import needs attention keeps the dialog open with the warnings.
  await page.getByRole("button", { name: "Import", exact: true }).click();
  await d.getByLabel("File").setInputFiles({
    name: "smoke.hurl",
    mimeType: "text/plain",
    buffer: Buffer.from('GET {{base_url}}/health\nHTTP 200\n[Asserts]\nxpath "//p" exists\n'),
  });
  await d.getByLabel("Name").fill("Smoke E2E");
  await d.getByRole("button", { name: "Import", exact: true }).click();
  await expect(d.getByRole("status")).toContainText("Imported Smoke E2E into smoke-e2e: 0 requests, 0 folders, 1 flow");
  await expect(d.getByText("xpath")).toBeVisible();
  await d.getByRole("button", { name: "Done" }).click();
  await expect(d).toBeHidden();
  expect(existsSync(`${WORKSPACE}/smoke-e2e/flow.flow.toml`)).toBe(true);

  // Errors are shown in the dialog.
  await page.getByRole("button", { name: "Import", exact: true }).click();
  await d.getByLabel("…or paste").fill("this is not an export");
  await d.getByRole("button", { name: "Import", exact: true }).click();
  await expect(d.getByRole("alert")).toContainText("unrecognized input");
  await d.getByRole("button", { name: "Close" }).click();

  // Clean up, so later tests see the workspace as before.
  const tree = page.getByRole("tree", { name: "Collections" });
  for (const name of ["api.example.com", "Smoke E2E"]) {
    await tree.getByRole("button", { name: `Actions for ${name}`, exact: true }).click();
    await page.getByRole("menuitem", { name: "Delete collection…" }).click();
    await page.getByRole("dialog").getByRole("button", { name: "Delete", exact: true }).click();
  }
  await expect.poll(() => existsSync(`${WORKSPACE}/pasted-e2e`) || existsSync(`${WORKSPACE}/smoke-e2e`)).toBe(false);
});
