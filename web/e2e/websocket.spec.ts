import { expect, test } from "@playwright/test";
import { fill, MOCK } from "./helpers";

test("a WebSocket request connects through the server and logs messages", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New request" }).first().click();
  await fill(page, "URL", `${MOCK.replace(/^http/, "ws")}/ws`);
  // The builder switches to WebSocket mode.
  await expect(page.getByRole("button", { name: "Connect" })).toBeVisible();
  await expect(page.getByRole("tab", { name: /Saved messages/ })).toBeVisible();
  await expect(page.getByRole("tab", { name: /Body/ })).toHaveCount(0);
  await page.getByRole("tab", { name: /Headers/ }).click();
  await page.getByRole("button", { name: "+ Add" }).click();
  await page.getByLabel("Headers name 1").fill("X-Token");
  await fill(page, "Headers value 1", "abc");

  await page.getByRole("button", { name: "Connect" }).click();
  const pane = page.getByRole("region", { name: "WebSocket" });
  await expect(pane.getByTestId("ws-status")).toHaveText("Connected");
  const log = pane.getByRole("list", { name: "Message log" });
  await expect(log).toContainText('{"hello":"abc"}');

  await pane.getByRole("textbox", { name: "Message to send" }).fill("ping from the UI");
  await pane.getByRole("button", { name: "Send" }).click();
  await expect(log.getByRole("listitem")).toHaveCount(3);
  await expect(log.getByRole("listitem").nth(1)).toContainText("ping from the UI");
  await expect(log.getByRole("listitem").nth(2)).toContainText("ping from the UI");

  // JSON messages can be shown pretty-printed.
  await log.getByRole("button", { name: "pretty" }).first().click();
  await expect(log.locator("pre").first()).toContainText('"hello": "abc"');

  await pane.getByRole("tab", { name: /Handshake/ }).click();
  await expect(pane.getByRole("table", { name: "Handshake response headers" })).toContainText("websocket");

  await page.getByRole("button", { name: "Disconnect" }).click();
  await expect(pane.getByTestId("ws-status")).toHaveText(/closed/i);
  await expect(page.getByRole("button", { name: "Connect" })).toBeVisible();
});
