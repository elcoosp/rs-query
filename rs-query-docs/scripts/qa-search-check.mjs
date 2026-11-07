// Extra hardening for the eyeball pass: opens the Pagefind search modal on the
// home page, types a domain key term, asserts that results come back, and
// saves a screenshot for manual inspection (.qa/search-modal.png).
import { chromium } from "playwright";
import { mkdir, writeFile } from "node:fs/promises";

const BASE = process.env.QA_BASE_URL ?? "http://localhost:4321";
await mkdir(".qa", { recursive: true });

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
const page = await ctx.newPage();

const failures = [];
page.on("pageerror", (err) => failures.push(`pageerror: ${err.message}`));
page.on("response", (res) => {
        if (res.status() >= 400) failures.push(`http ${res.status()} ${res.url()}`);
});

await page.goto(BASE + "/", { waitUntil: "networkidle" });
await page.evaluate(() => document.fonts.ready);

// Starlight's search button lives in the header.
const trigger = page.locator("button[aria-label='Search'], starlight-menu-button, .search-button, button[data-open-search]").first();
if ((await trigger.count()) === 0) {
        // Fallback: keyboard shortcut `Cmd/Ctrl+K` opens the Pagefind modal.
        await page.keyboard.press("ControlOrMeta+k");
} else {
        await trigger.click();
}
await page.waitForTimeout(400);

const input = page.locator("#search-input, input[type='text']").first();
if ((await input.count()) === 0) {
        failures.push("search modal did not open (no input found)");
} else {
        await input.fill("invalidate");
        await page.waitForTimeout(900); // Pagefind debounce + wasm search
        const results = await page.evaluate(() => {
                const items = document.querySelectorAll(".pagefind-ui__result");
                return {
                        count: items.length,
                        titles: [...items].slice(0, 5).map((el) => el.innerText.slice(0, 80)),
                };
        });
        console.log("pagefind results:", JSON.stringify(results, null, 2));
        if (results.count === 0) failures.push("pagefind returned 0 results for 'invalidate'");
}

await page.screenshot({ path: ".qa/search-modal.png", fullPage: false });

// Also capture the custom 404 once (excluded from the route walk by design).
const page404 = await ctx.newPage();
await page404.goto(BASE + "/this-route-does-not-exist/", { waitUntil: "networkidle" });
await page404.screenshot({ path: ".qa/404.png", fullPage: true });
await page404.close();

await writeFile(".qa/search-failures.json", JSON.stringify(failures, null, 2));
await browser.close();

if (failures.length) {
        console.error(`SEARCH QA FAILED — ${failures.length} issue(s):`);
        for (const f of failures) console.error(`  ${f}`);
        process.exit(1);
}
console.log("SEARCH QA PASSED");
