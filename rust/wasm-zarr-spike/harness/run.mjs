// Headless driver for harness/index.html — serves the crate directory over
// http://127.0.0.1 (OPFS requires a secure context; localhost is one) and reads
// the worker's result off the page. Exits non-zero if any check failed.
//
//   node harness/run.mjs [path/to/playwright]
//
// Playwright is not a dependency of this crate; pass the module path or set
// PLAYWRIGHT_MODULE. Any Chromium-based automation works — this is a plain page.

import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const crateDir = normalize(join(fileURLToPath(import.meta.url), "..", ".."));
const TYPES = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".wasm": "application/wasm",
  ".json": "application/json",
};

const server = createServer(async (req, res) => {
  const path = decodeURIComponent(new URL(req.url, "http://x").pathname);
  const file = join(crateDir, path === "/" ? "/harness/index.html" : path);
  if (!file.startsWith(crateDir)) {
    res.writeHead(403).end();
    return;
  }
  try {
    const body = await readFile(file);
    res.writeHead(200, { "content-type": TYPES[extname(file)] ?? "application/octet-stream" });
    res.end(body);
  } catch {
    res.writeHead(404).end();
  }
});

await new Promise((r) => server.listen(0, "127.0.0.1", r));
const port = server.address().port;

const playwrightPath = process.argv[2] ?? process.env.PLAYWRIGHT_MODULE ?? "playwright";
const { chromium } = await import(playwrightPath);
const browser = await chromium.launch();
const page = await browser.newPage();
page.on("console", (m) => console.log(`[page] ${m.text()}`));
await page.goto(`http://127.0.0.1:${port}/harness/index.html`);
await page.waitForFunction(() => window.__spike__ !== undefined, null, { timeout: 60_000 });
const out = await page.evaluate(() => window.__spike__);
await browser.close();
server.close();

console.log(JSON.stringify(out, null, 2));
const failed = out.error || (out.results ?? []).some((r) => !r.ok) || !out.results?.length;
process.exit(failed ? 1 : 0);
