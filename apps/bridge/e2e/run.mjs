// End-to-end tests of the Trurl desktop UI against the REAL core and REAL models.
//
//   node run.mjs --url http://127.0.0.1:7861 --suite full --out ../../../target/e2e/linux
//   node run.mjs --cdp http://127.0.0.1:9222 --suite smoke --out ...   (the installed Windows app)
//
// Suites:
//   smoke    load, chat, stop. Used on the real Windows app.
//   full     every learner flow, with screenshots for the README.
//   offline  the Hugging Face tab when there is no internet.
// Every step asserts on what is on screen, the way a learner would see it.

import { chromium } from "playwright";
import fs from "node:fs";
import path from "node:path";

const arg = (name, def) => {
  const i = process.argv.indexOf("--" + name);
  return i > 0 ? process.argv[i + 1] : def;
};
const URL_ = arg("url");
const CDP = arg("cdp");
const SUITE = arg("suite", "smoke");
const OUT = arg("out", "../../../target/e2e/" + SUITE);
const W = Number(arg("width", 1366));
const H = Number(arg("height", 768));
fs.mkdirSync(OUT, { recursive: true });

let failures = 0;
let shotN = 0;
const log = (...a) => console.log(new Date().toISOString().slice(11, 19), ...a);

async function step(name, fn) {
  const t0 = Date.now();
  try {
    await fn();
    log(`ok   ${name} (${((Date.now() - t0) / 1000).toFixed(1)}s)`);
  } catch (e) {
    failures++;
    log(`FAIL ${name}: ${e.message.split("\n")[0]}`);
    await shot(page, "fail-" + name.replace(/\W+/g, "-")).catch(() => {});
  }
}

async function shot(p, name) {
  shotN++;
  const file = path.join(OUT, `${String(shotN).padStart(2, "0")}-${name}.png`);
  await p.screenshot({ path: file });
  return file;
}

const consoleErrors = [];
const external = [];
let browser, page;

if (CDP) {
  browser = await chromium.connectOverCDP(CDP, { timeout: 60000 });
  const ctx = browser.contexts()[0];
  page = ctx.pages().find((p) => !p.url().startsWith("devtools")) || ctx.pages()[0];
} else {
  browser = await chromium.launch();
  page = await browser.newPage({ viewport: { width: W, height: H } });
  await page.goto(URL_);
}
page.on("console", (m) => m.type() === "error" && consoleErrors.push(m.text()));
page.on("request", (r) => {
  const u = r.url();
  if (!/^(https?:\/\/(127\.0\.0\.1|localhost|tauri\.localhost|ipc\.localhost)|data:|blob:|file:|tauri:|ipc:)/.test(u)) external.push(u);
});

const pill = () => page.locator("#pill").innerText();
const waitPill = (text, timeout = 300000) => page.waitForFunction((t) => document.querySelector("#pill")?.innerText.includes(t), text, { timeout });
const lastTurn = () => page.locator(".turn").last();

async function ask(text, { waitDone = true } = {}) {
  await page.fill("#input", text);
  await page.click("#send");
  await page.locator(".turn").filter({ hasText: text }).last().waitFor({ timeout: 30000 });
  if (waitDone) await waitPill("Ready", 600000);
}

async function openModels(tab = "local") {
  if (!(await page.locator("#modelsDlg[open]").count())) await page.click("#openModels");
  await page.click(tab === "hub" ? "#tabHub" : "#tabLocal");
}
const closeDialogs = () => page.evaluate(() => document.querySelectorAll("dialog[open]").forEach((d) => d.close()));

async function useModel(name) {
  await openModels("local");
  const row = page.locator("#modelList li").filter({ hasText: name });
  if (await row.getByRole("button", { name: `Use ${name}` }).count()) await row.getByRole("button", { name: `Use ${name}` }).click();
  else await closeDialogs();
  // Wait for the NEW model to be ready, not the old one still showing Ready.
  await page.waitForFunction(
    (n) => document.querySelector("#chatTitle")?.innerText.includes(n) && document.querySelector("#pill")?.innerText.includes("Ready"),
    name,
    { timeout: 300000 }
  );
}

// ----- suites ---------------------------------------------------------------

await step("model loads and shows Ready", async () => {
  await waitPill("Ready", 300000);
  await shot(page, "ready");
});

if (SUITE === "smoke" || SUITE === "full") {
  await step("chat: an answer streams in", async () => {
    await ask("What is the capital of Ghana? Answer in one short sentence.");
    const answer = await lastTurn().locator(".md").innerText();
    if (!/accra/i.test(answer)) throw new Error("answer does not mention Accra: " + answer);
    await shot(page, "chat");
  });

  await step("stop mid-answer, then chat again", async () => {
    await ask("Write a long story about a goat who learns to read.", { waitDone: false });
    await page.waitForFunction(() => (document.querySelector(".turn:last-of-type .md")?.innerText || "").length > 40, null, { timeout: 120000 });
    await page.keyboard.press("Escape");
    await lastTurn().locator(".end-note", { hasText: "Stopped" }).waitFor({ timeout: 30000 });
    await waitPill("Ready", 60000);
    await ask("Thanks. Say hello in French.");
    if (!(await lastTurn().locator(".md").innerText()).trim()) throw new Error("no answer after Stop");
  });
}

if (SUITE === "full") {
  await step("new chat clears the conversation", async () => {
    await page.keyboard.press("Control+n");
    await page.waitForFunction(() => !document.querySelector(".turn"), null, { timeout: 10000 });
  });

  await step("reasoning: thinking streams live, folds, says how long", async () => {
    await useModel("Qwen3 0.6B");
    // A 0.6B model sometimes thinks until the context is full: allow three tries.
    for (let attempt = 0; attempt < 3; attempt++) {
      if (attempt) await page.keyboard.press("Control+n");
      await ask("What is 17 + 25? Reply with the number only.", { waitDone: false });
      await page.locator(".think[data-live]").waitFor({ timeout: 120000 });
      await page.waitForFunction(() => (document.querySelector(".think-body")?.innerText || "").length > 60, null, { timeout: 120000 });
      if (!attempt) await shot(page, "thinking");
      await waitPill("Ready", 600000);
      if ((await lastTurn().locator(".md").innerText()).trim()) break;
    }
    const label = await lastTurn().locator(".think-label").innerText();
    if (!/^Thought for \d/.test(label)) throw new Error("label is " + label);
    if ((await lastTurn().locator(".think-toggle").getAttribute("aria-expanded")) !== "false") throw new Error("thinking did not fold");
    if (!/42/.test(await lastTurn().locator(".md").innerText())) throw new Error("wrong answer");
    await shot(page, "thought");
    await lastTurn().locator(".think-toggle").click();
    await shot(page, "thought-open");
  });

  await step("/no_think leaves no thinking block", async () => {
    await ask("/no_think Say OK.");
    if (await lastTurn().locator(".think").count()) throw new Error("a thinking block appeared");
  });

  await step("controls: system prompt and temperature 0", async () => {
    await page.keyboard.press("Control+n");
    await useModel("Gemma 3 1B Instruct");
    await page.fill("#sysPrompt", "You are a friendly pirate. Always start your answer with the word Arr.");
    await page.locator("#sysPrompt").blur();
    await page.locator("#temp").fill("0");
    await page.locator("#temp").dispatchEvent("change");
    await page.waitForFunction(() => document.querySelector("#tempOut").textContent === "0.0");
    await ask("Hello! Who are you?");
    const a = (await lastTurn().locator(".md").innerText()).trim();
    if (!/^arr/i.test(a)) throw new Error("system prompt not followed: " + a.slice(0, 80));
    await shot(page, "controls");
  });

  await step("temperature 0 gives the same answer twice", async () => {
    await page.keyboard.press("Control+n");
    await ask("Name one fruit. One word.");
    const a = await lastTurn().locator(".md").innerText();
    await page.keyboard.press("Control+n");
    await ask("Name one fruit. One word.");
    const b = await lastTurn().locator(".md").innerText();
    if (a !== b) throw new Error(`different answers: ${a} / ${b}`);
    await page.click("#sysClear");
    await page.locator("#tempReset").click();
  });

  await step("context 256: the conversation runs out of room", async () => {
    await page.keyboard.press("Control+n");
    await page.selectOption("#ctx", "256");
    await waitPill("Loading", 30000).catch(() => {});
    await waitPill("Ready", 300000);
    await ask("Please repeat this text back to me exactly: " + "the quick brown fox jumps over the lazy dog. ".repeat(40));
    const note = await lastTurn().locator(".inline-note").innerText();
    if (!note.includes("The conversation ran out of room.")) throw new Error("no out-of-room message: " + note);
    await shot(page, "out-of-room");
    await page.locator("#ctxReset").click();
    await waitPill("Ready", 300000);
  });

  await step("models screen", async () => {
    await openModels("local");
    await page.locator("#modelList li").first().waitFor();
    if ((await page.locator("#modelList li").count()) < 2) throw new Error("expected the two test models");
    await shot(page, "models");
  });

  await step("Hugging Face: search, details, download, verify, install", async () => {
    await openModels("hub");
    await page.locator("#hubQuery").waitFor({ timeout: 30000 });
    await page.fill("#hubQuery", "test-model-stories260K");
    await page.keyboard.press("Enter");
    await page.locator(".hit", { hasText: "ggml-org/test-model-stories260K" }).first().click();
    await page.locator(".files tbody tr").first().waitFor({ timeout: 30000 });
    await shot(page, "hf-details-tiny");
    await page.getByRole("button", { name: /Download F32/ }).click();
    // Downloaded, SHA-256 checked and installed: the row says so, and the model is in the list.
    await page.locator(".files tbody tr .tag-held", { hasText: "On this computer" }).waitFor({ timeout: 120000 });
    if (!/\d/.test(await page.locator("#tabLocal .num").innerText())) throw new Error("model count missing");
    await page.click(".back");
  });

  await step("Hugging Face: a real model's details page", async () => {
    await page.fill("#hubQuery", "Qwen3 0.6B GGUF");
    await page.keyboard.press("Enter");
    await page.locator(".hit").first().waitFor({ timeout: 30000 });
    await shot(page, "hf-results");
    await page.locator(".hit", { hasText: "bartowski/Qwen_Qwen3-0.6B-GGUF" }).first().click();
    await page.locator(".files tbody tr").first().waitFor({ timeout: 30000 });
    await page.waitForTimeout(500);
    await shot(page, "hf-details");
    await closeDialogs();
  });

  await step("about and licenses", async () => {
    await page.click("#openAbout");
    await page.locator(".licenses details").first().waitFor({ timeout: 10000 });
    const text = await page.locator("#aboutBody").innerText();
    if (!/Gemma Terms of Use/.test(text) || !/Cyberiad Labs/.test(text)) throw new Error("about is missing licenses or attribution");
    await shot(page, "about");
    await closeDialogs();
  });
}

if (SUITE === "update") {
  // The installed version N sees N+1 in the manifest and updates when asked.
  const expect = arg("expect-version");
  await step(`update card offers version ${expect}`, async () => {
    await page.getByText(`Update available: version ${expect}`).waitFor({ timeout: 120000 });
    await shot(page, "update-offered");
  });
  await step("Update now downloads, installs and closes the app", async () => {
    const gone = new Promise((resolve) => browser.on("disconnected", resolve));
    await page.getByRole("button", { name: "Update now" }).click();
    await Promise.race([gone, new Promise((_, rej) => setTimeout(() => rej(new Error("the app did not close to install")), 180000))]);
  });
  log(failures ? `${failures} step(s) FAILED` : "all steps passed");
  process.exit(failures ? 1 : 0);
}

if (SUITE === "after-update") {
  const expect = arg("expect-version");
  await step(`About says version ${expect}`, async () => {
    await page.click("#openAbout");
    await page.getByText(`Version ${expect}`).waitFor({ timeout: 10000 });
    await shot(page, "about-after-update");
    await closeDialogs();
  });
  await step("the model kept through the update still answers", async () => {
    await ask("/no_think Say OK.");
    if (!(await lastTurn().locator(".md").innerText()).trim()) throw new Error("no answer after the update");
  });
}

if (SUITE === "offline") {
  await step("Hugging Face says to connect, everything else works", async () => {
    await openModels("hub");
    await page.getByText("Connect to the internet to browse Hugging Face").waitFor({ timeout: 30000 });
    await shot(page, "hf-offline");
    await closeDialogs();
    await ask("Say hello.");
  });
}

await step("no console errors", async () => {
  if (consoleErrors.length) throw new Error(consoleErrors.join(" | "));
});
await step("no request left this computer", async () => {
  // The bridge proxies Hugging Face through the core; the page itself must never fetch the internet.
  if (external.length) throw new Error(external.join(", "));
});

if (!CDP) await browser.close();
else await browser.close().catch(() => {});
log(failures ? `${failures} step(s) FAILED` : "all steps passed", "screenshots in", OUT);
process.exit(failures ? 1 : 0);
