import { chromium } from "playwright";
import { readdir, mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

const BASE = process.env.QA_BASE_URL ?? "http://localhost:4321";
const OUT = ".qa/screenshots";

async function walk(dir) {
        const entries = await readdir(dir, { withFileTypes: true });
        const files = [];
        for (const e of entries) {
                const p = path.join(dir, e.name);
                if (e.isDirectory()) files.push(...(await walk(p)));
                else if (e.name === "index.html") files.push(p);
        }
        return files;
}

async function routes() {
        const files = await walk("dist");
        return files.map((f) => {
                const rel = path.relative("dist", path.dirname(f));
                // path.relative returns "" (not ".") for identical paths.
                return "/" + (rel === "." || rel === "" ? "" : rel + "/");
        });
}

const VIEWPORTS = {
        desktop: { width: 1440, height: 900 },
        mobile: { width: 390, height: 844 },
};

const failures = [];

function fail(route, viewport, theme, kind, detail) {
        failures.push({ route, viewport, theme, kind, detail });
}

const browser = await chromium.launch();
const all = await routes();
await mkdir(OUT, { recursive: true });

for (const route of all) {
        for (const [vpName, viewport] of Object.entries(VIEWPORTS)) {
                for (const theme of ["light", "dark"]) {
                        const ctx = await browser.newContext({
                                viewport,
                                reducedMotion: "reduce",
                        });
                        const page = await ctx.newPage();

                        page.on("console", (msg) => {
                                if (msg.type() === "error")
                                        fail(route, vpName, theme, "console", msg.text());
                        });
                        page.on("pageerror", (err) =>
                                fail(route, vpName, theme, "pageerror", err.message),
                        );
                        page.on("response", (res) => {
                                if (res.status() >= 400)
                                        fail(route, vpName, theme, "http", `${res.status()} ${res.url()}`);
                        });

                        await page.addInitScript((t) => {
                                try {
                                        localStorage.setItem("starlight-theme", t);
                                } catch {}
                        }, theme);

                        await page.goto(BASE + route, { waitUntil: "networkidle" });
                        await page.evaluate(() => document.fonts.ready);

                        // Verify the theme actually applied; fall back to the toggle if not.
                        const applied = await page.evaluate(
                                () => document.documentElement.dataset.theme,
                        );
                        if (applied !== theme) {
                                const toggle = page
                                        .locator("starlight-theme-select button, button[data-theme-toggle]")
                                        .first();
                                if (await toggle.count()) {
                                        await toggle.click();
                                        await page.waitForTimeout(150);
                                }
                        }

                        const probe = await page.evaluate(() => {
                                const main = document.querySelector("main");
                                const imgs = [...document.querySelectorAll("img")];
                                return {
                                        theme: document.documentElement.dataset.theme,
                                        h1: document.querySelectorAll("h1").length,
                                        textLen: (main?.innerText ?? "").trim().length,
                                        overflow:
                                                document.documentElement.scrollWidth > window.innerWidth + 1,
                                        brokenImgs: imgs
                                                .filter((i) => !i.complete || i.naturalWidth === 0)
                                                .map((i) => i.currentSrc || i.src),
                                };
                        });

                        if (probe.theme !== theme)
                                fail(route, vpName, theme, "theme", `applied=${probe.theme}`);
                        if (probe.h1 !== 1) fail(route, vpName, theme, "h1", `count=${probe.h1}`);
                        if (probe.textLen < 200)
                                fail(
                                        route,
                                        vpName,
                                        theme,
                                        "empty",
                                        `main text length=${probe.textLen}`,
                                );
                        if (probe.overflow)
                                fail(route, vpName, theme, "overflow", "horizontal scrollbar present");
                        for (const src of probe.brokenImgs)
                                fail(route, vpName, theme, "image", src);

                        const slug =
                                route === "/"
                                        ? "index"
                                        : route.replace(/^\/|\/$/g, "").replace(/\//g, "_");
                        await page.screenshot({
                                path: path.join(OUT, `${slug}--${vpName}--${theme}.png`),
                                fullPage: true,
                        });

                        await ctx.close();
                }
        }
}

await browser.close();
await writeFile(".qa/failures.json", JSON.stringify(failures, null, 2));

if (failures.length) {
        console.error(`QA FAILED — ${failures.length} issue(s):`);
        for (const f of failures)
                console.error(
                        `  [${f.kind}] ${f.route} (${f.viewport}/${f.theme}): ${f.detail}`,
                );
        process.exit(1);
}
console.log(`QA PASSED — ${all.length} routes × 2 viewports × 2 themes`);
