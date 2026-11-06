import { expect, test } from "@playwright/test";
import { fill, MOCK } from "./helpers";

test("a GraphQL request is sent as JSON and its schema can be browsed", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "New request" }).first().click();
  await fill(page, "URL", `${MOCK}/graphql`);
  await page.getByRole("tab", { name: /Body/ }).click();
  await page.getByRole("radio", { name: "GraphQL" }).check();
  // GraphQL goes with POST.
  await expect(page.getByRole("combobox", { name: "Method" })).toHaveValue("POST");
  await fill(page, "GraphQL query", "query Me($id: ID) { me { id name } }");
  await fill(page, "GraphQL variables", '{ "id": "1" }');
  await page.getByRole("textbox", { name: "Operation name" }).fill("Me");
  await page.getByRole("button", { name: "Send" }).click();
  const response = page.getByRole("region", { name: "Response" });
  await expect(response).toContainText("200");
  await expect(response).toContainText('"name": "Demo"');
  await expect(response).toContainText('"operationName": "Me"');

  await page.getByRole("button", { name: "Fetch schema" }).click();
  await page.getByRole("button", { name: "Browse schema" }).click();
  const schema = page.getByRole("region", { name: "GraphQL schema" });
  await expect(schema).toContainText("query: Query");
  await schema.getByRole("button", { name: /Query$/ }).click();
  await expect(schema).toContainText("user(id: ID!):");
  await schema.getByRole("button", { name: "User", exact: true }).first().click();
  await expect(schema.locator("#gql-type-User")).toContainText("role:");
  await schema.getByRole("searchbox", { name: "Find in the schema" }).fill("price");
  await expect(schema.getByRole("list", { name: "Schema types" }).getByRole("listitem").first()).toContainText("Product");
});
