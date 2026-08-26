// SPDX-FileCopyrightText: 2026 Minamiyama Kotaro
// SPDX-License-Identifier: AGPL-3.0-only
//
// Renders one standalone HTML file (a `wrap_grid_page`/`grid.rs` split
// grid page, written by `xlsxdiff --grid-html-dir`) to a full-page PNG
// screenshot, for `action.yml`'s `visual` mode. A plain
// `page.goto('file://...')` + `page.screenshot()` — the Playwright CLI's
// own `screenshot` subcommand doesn't reliably document `file://` +
// full-page support, so this goes through the programmatic API directly
// instead.
//
// usage: node screenshot.mjs <input.html> <output.png>

import { chromium } from "playwright";
import { pathToFileURL } from "node:url";

const [, , inputHtml, outputPng] = process.argv;
if (!inputHtml || !outputPng) {
  console.error("usage: node screenshot.mjs <input.html> <output.png>");
  process.exit(1);
}

const browser = await chromium.launch();
try {
  const page = await browser.newPage();
  await page.goto(pathToFileURL(inputHtml).href);
  // An element screenshot crops to that element's own rendered box,
  // rather than the viewport (page.screenshot) — `wrap_grid_page`
  // (src/grid.rs) wraps its content in `.page-content`
  // (`display: inline-block`) specifically so this box shrinks to fit
  // the actual grid instead of stretching to the default 1280px
  // viewport width, which a sheet's own (variable, content-dependent)
  // width would otherwise leave mostly blank.
  await page.locator(".page-content").screenshot({ path: outputPng });
} finally {
  await browser.close();
}
