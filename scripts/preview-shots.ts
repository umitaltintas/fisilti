// Screenshots of the main window against the fake backend, for design review.
// Start `bun run dev:preview` first, then: bun scripts/preview-shots.ts [outDir]
import { chromium } from "@playwright/test";

const BASE = "http://localhost:1430";
const out = process.argv[2] ?? "/tmp/fisilti-shots";
const LANG = process.env.SHOT_LANG ?? "tr";

const sections: [name: string, label: RegExp][] = [
  ["home", /Ana sayfa|^Home$/],
  ["general", /^Genel$|^General$/],
  ["models", /Modeller|Models/],
  ["meetings", /Toplantılar|Meetings|^Meeting$/],
  ["advanced", /Gelişmiş|Advanced/],
];

const only = process.env.SHOT_ONLY?.split(",");

// Any installed Chromium works (CDP is version tolerant); set
// SHOT_BROWSER to its executable when Playwright's own download is missing.
const browser = await chromium.launch({
  executablePath: process.env.SHOT_BROWSER || undefined,
});
for (const scheme of ["light", "dark"] as const) {
  for (const state of ["idle", "recording"]) {
    const page = await browser.newPage({
      viewport: { width: 880, height: 640 },
      colorScheme: scheme,
      deviceScaleFactor: 2,
      locale: LANG === "tr" ? "tr-TR" : "en-US",
    });
    page.on("pageerror", (e) => console.error(`[${scheme}/${state}] pageerror`, e.message));
    await page.goto(`${BASE}/?lang=${LANG}&state=${state}`);
    await page.waitForTimeout(1200);
    for (const [name, label] of sections) {
      if (state === "recording" && !["home", "meetings"].includes(name)) continue;
      if (only && !only.includes(name)) continue;
      await page.getByRole("navigation").getByRole("button", { name: label }).first().click();
      await page.waitForTimeout(500);
      const file = `${out}/${name}-${state}-${scheme}.png`;
      await page.screenshot({ path: file });
      console.log(file);
    }
    await page.close();
  }
}
await browser.close();
