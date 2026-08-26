import { expect, test } from "@playwright/test";
import path from "node:path";

const screenshotDirectory = process.env.FIXTRACE_SCREENSHOT_DIR;

async function capture(page: import("@playwright/test").Page, name: string) {
  if (!screenshotDirectory) return;
  await page.screenshot({ path: path.join(screenshotDirectory, name), fullPage: true });
}

test("complete desktop mock workflow", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "parser-repair", level: 1 })).toBeVisible();

  await page.getByRole("button", { name: "+ New session" }).click();
  const newSession = page.getByRole("dialog", { name: "New verified session" });
  await newSession.getByPlaceholder("/path/to/project").fill("/tmp/fixtrace-e2e");
  await newSession.getByText("Oracle command").locator("..").getByRole("textbox").fill("cargo test");
  await newSession.getByText("Session title").locator("..").getByRole("textbox").fill("e2e-repair");
  await newSession.getByRole("button", { name: "Create session" }).click();
  await expect(page.getByRole("heading", { name: "e2e-repair", level: 1 })).toBeVisible();

  const composer = page.getByRole("textbox", { name: "Message FixTrace" });
  await composer.fill("Find and verify the minimal repair.");
  await composer.press("Enter");
  await expect(page.getByRole("button", { name: "Cancel" })).toBeVisible();
  await expect(page.getByRole("button", { name: /run_candidate/ })).toBeVisible();

  const approval = page.getByRole("dialog", { name: "Run recorded Oracle command" });
  await expect(approval).toBeVisible();
  await capture(page, "desktop-approval-mock.png");
  await approval.getByRole("button", { name: "Approve once" }).click();
  await page.getByRole("button", { name: /run_candidate/ }).click();
  await expect(page.getByRole("code", { name: "" })).toContainText(
    "actions=[5, 6] repetitions=3",
  );
  await expect(page.getByText("Verified trial")).toBeVisible();
  await page.getByRole("button", { name: "Cancel" }).click();
  await expect(page.getByText("Task cancelled")).toBeVisible();
  await capture(page, "desktop-overview-mock.png");

  await page.getByRole("button", { name: /parser-repair/ }).click();
  await expect(page.getByRole("heading", { name: "parser-repair", level: 1 })).toBeVisible();
  await page.getByRole("tab", { name: "Settings" }).click();
  await expect(page.getByRole("group", { name: "Model" })).toBeVisible();
  await page.getByLabel("Theme").selectOption("light");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await page.getByRole("textbox", { name: "Model" }).fill("glm-5-e2e");
  await page.getByRole("button", { name: "Save settings" }).click();
  await expect(page.getByText("glm-5-e2e", { exact: true }).first()).toBeVisible();
  await page.getByRole("button", { name: "Test connection" }).click();
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Export", exact: true }).click();
  const exportDialog = page.getByRole("dialog", { name: "Export session" });
  await exportDialog.getByPlaceholder("/path/to/session.json").fill("/tmp/fixtrace-e2e.json");
  await exportDialog.getByRole("button", { name: "Export" }).click();
  await expect(exportDialog).toBeHidden();
});

test("virtualizes a recovered 10,000 item timeline", async ({ page }) => {
  await page.goto("/?timeline_items=10000");
  await expect(page.getByRole("heading", { name: "parser-repair", level: 1 })).toBeVisible();
  await expect(page.getByText("10,000 items")).toBeVisible();
  await expect.poll(() => page.locator(".timeline-card").count()).toBeGreaterThan(0);
  expect(await page.locator(".timeline-card").count()).toBeLessThan(100);
});
