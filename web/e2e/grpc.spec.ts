import { expect, test } from "@playwright/test";
import { fill, GRPC } from "./helpers";

test("a gRPC request lists services by reflection and calls unary and streaming methods", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New request" }).first().click();
  await fill(page, "URL", `grpc://${GRPC}`);
  await expect(page.getByRole("button", { name: "Invoke" })).toBeVisible();
  const builder = page.getByRole("region", { name: "Request" });
  await builder.getByRole("tab", { name: /Metadata/ }).click();
  await page.getByRole("button", { name: "+ Add" }).click();
  await page.getByLabel("Headers name 1").fill("x-user");
  await fill(page, "Headers value 1", "e2e");

  await builder.getByRole("tab", { name: /Call/ }).click();
  await page.getByRole("button", { name: "Load services" }).click();
  const services = page.getByRole("group", { name: "Services" });
  await expect(services).toContainText("shop.v1.Orders");
  await services.getByRole("button", { name: "Get", exact: true }).click();
  await expect(page.getByText("unary: shop.v1.GetOrder → shop.v1.Order")).toBeVisible();
  await page.getByRole("button", { name: "Insert template" }).click();
  await fill(page, "gRPC message", '{ "id": "7" }');
  await page.getByRole("button", { name: "Invoke" }).click();
  const pane = page.getByRole("region", { name: "gRPC response" });
  await expect(pane.getByTestId("grpc-status")).toHaveText("OK");
  const messages = pane.getByRole("list", { name: "Response messages" });
  await expect(messages).toContainText('"item": "book for e2e"');
  await pane.getByRole("tab", { name: "Metadata" }).click();
  await expect(pane.getByRole("table", { name: "Response headers" })).toContainText("mock-grpc");

  // Errors are statuses.
  await fill(page, "gRPC message", '{ "id": "404" }');
  await page.getByRole("button", { name: "Invoke" }).click();
  await expect(pane.getByTestId("grpc-status")).toHaveText("NOT_FOUND");
  await expect(pane).toContainText("order 404 does not exist");

  // Server streaming.
  await services.getByRole("button", { name: "Watch" }).click();
  await fill(page, "gRPC message", '{ "id": "1" }');
  await page.getByRole("button", { name: "Invoke" }).click();
  await expect(pane.getByTestId("grpc-status")).toHaveText("OK");
  await pane.getByRole("tab", { name: /Messages/ }).click();
  await expect(messages.getByRole("listitem")).toHaveCount(3);

  // Bidirectional streaming: more messages while the call runs, then the end of the stream.
  await services.getByRole("button", { name: "Chat" }).click();
  await fill(page, "gRPC message", '[{ "item": "one" }]');
  await page.getByRole("button", { name: "Invoke" }).click();
  await expect(messages).toContainText('"item": "ONE"');
  await pane.getByRole("textbox", { name: "Next message" }).fill('{ "item": "two" }');
  await pane.getByRole("button", { name: "Send" }).click();
  await expect(messages).toContainText('"item": "TWO"');
  await pane.getByRole("button", { name: "End stream" }).click();
  await expect(pane.getByTestId("grpc-status")).toHaveText("OK");
});
