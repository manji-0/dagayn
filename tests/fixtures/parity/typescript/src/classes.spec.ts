import { Box } from "./classes";
import { test, expect } from "@playwright/test";
test("playwright box", async ({ page }) => {
  Box.create();
});
test.describe("group", () => {
  test("inner", () => {});
});
