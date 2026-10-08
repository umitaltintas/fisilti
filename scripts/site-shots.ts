// Product screenshots for the GitHub Pages site (docs/screens), rendered
// against the fake backend. Start `bun run dev:preview` first, then:
//   SHOT_BROWSER=<chromium> bun scripts/site-shots.ts
import { chromium } from "@playwright/test";

const BASE = "http://localhost:1430";
const browser = await chromium.launch({
  executablePath: process.env.SHOT_BROWSER || undefined,
});

const shoot = async (
  file: string,
  scheme: "light" | "dark",
  state: string,
  section: RegExp,
  notes?: string,
) => {
  const page = await browser.newPage({
    viewport: { width: 880, height: 600 },
    colorScheme: scheme,
    deviceScaleFactor: 2,
    locale: "en-US",
  });
  await page.goto(`${BASE}/?lang=en&state=${state}`);
  await page.waitForTimeout(1200);
  await page
    .getByRole("navigation")
    .getByRole("button", { name: section })
    .first()
    .click();
  await page.waitForTimeout(700);
  if (notes) {
    await page.locator("textarea").first().fill(notes);
    await page.locator("textarea").first().blur();
    await page.waitForTimeout(1200);
  }
  await page.screenshot({ path: `docs/screens/${file}` });
  console.log(file);
  await page.close();
};

await shoot("home-dark.png", "dark", "idle", /^Home$/);
await shoot(
  "meeting-light.png",
  "light",
  "recording",
  /^Meetings/,
  "Pricing settled by end of October.\n• Enterprise: three tiers, entry not too cheap\n• Summaries stay in Pro\n\nMobile moves out a quarter",
);
await browser.close();
