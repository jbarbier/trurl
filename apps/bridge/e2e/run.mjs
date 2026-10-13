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
  // The app opens its WebView2 debug port a few seconds after it starts:
  // "connection refused" until then is normal, so keep knocking for 90 s.
  const until = Date.now() + 90000;
  for (;;) {
    try {
      browser = await chromium.connectOverCDP(CDP, { timeout: 10000 });
      break;
    } catch (e) {
      if (Date.now() > until) throw e;
      await new Promise((r) => setTimeout(r, 1000));
    }
  }
  const ctx = browser.contexts()[0];
  while (!ctx.pages().some((p) => !p.url().startsWith("devtools"))) {
    if (Date.now() > until) throw new Error("the app window never appeared");
    await new Promise((r) => setTimeout(r, 500));
  }
  page = ctx.pages().find((p) => !p.url().startsWith("devtools"));
} else {
  browser = await chromium.launch();
  page = await browser.newPage({ viewport: { width: W, height: H }, permissions: ["clipboard-read", "clipboard-write"] });
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

// Switch models the way a learner does: the dropdown on the sidebar card.
async function useModel(name) {
  await closeDialogs();
  await page.click("#modelCard");
  await page.locator("#modelOptions .model-option", { hasText: name }).first().click();
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
      // Expanded while generating: the toggle says so and the text is really on screen.
      if (await page.locator(".think[data-live]").count()) {
        const live = lastTurn().locator(".think");
        if ((await live.locator(".think-toggle").getAttribute("aria-expanded")) !== "true") throw new Error("thinking is folded while it streams");
        if (!(await live.locator(".think-body").isVisible())) throw new Error("thinking text is hidden while it streams");
        // The status says so too (not "Starting" for the whole think).
        if (!/Thinking/.test(await pill())) throw new Error("the status pill says " + (await pill()) + " while the model thinks");
      }
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
    await page.waitForTimeout(400); // let the chevron finish turning
    await shot(page, "thought-open");
  });

  await step("/no_think leaves no thinking block", async () => {
    await ask("/no_think Say OK.");
    if (await lastTurn().locator(".think").count()) throw new Error("a thinking block appeared");
  });

  await step("Copy puts the answer on the clipboard", async () => {
    const answer = (await lastTurn().locator(".md").innerText()).trim();
    await lastTurn().getByRole("button", { name: "Copy this answer" }).click();
    await lastTurn().getByText("Copied").waitFor({ timeout: 5000 });
    const got = (await page.evaluate(() => navigator.clipboard.readText())).trim();
    if (!got || !answer.includes(got.replace(/[*_`#]/g, "").trim().slice(0, 10))) throw new Error(`clipboard has ${JSON.stringify(got)}, answer is ${JSON.stringify(answer)}`);
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
    if (!note.includes("Your message is too long for the context size.")) throw new Error("no too-long message: " + note);
    // The first message of a chat: a bigger context helps, a new chat does not.
    if (!(await lastTurn().getByRole("button", { name: /Use 512 tokens/ }).count())) throw new Error("no one-click bigger context");
    if (await lastTurn().getByRole("button", { name: "New chat" }).count()) throw new Error("New chat offered on the first message");
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

  await step("course models: listed first on the download screen, in course order", async () => {
    await openModels("hub");
    await page.locator(".course-row").nth(2).waitFor({ timeout: 30000 });
    const rows = await page.locator(".course-row").allInnerTexts();
    const names = rows.map((r) => r.split("\n")[0].trim());
    if (names.join(" | ") !== "Gemma 3 1B Instruct | Gemma 3 1B PT | Qwen3 0.6B") throw new Error("course rows: " + names.join(" | "));
    // The test home holds the Gemma 3 1B and Qwen3 0.6B files: those two are here, the PT one is not.
    const row = (id) => page.locator(`.course-row[data-course="${id}"]`);
    for (const id of ["gemma-3-1b-it", "qwen3-0.6b"]) if (!(await row(id).getByText("On this computer").count())) throw new Error(id + " not shown as on this computer");
    // The search box shows once the internet check answers; then Download is enabled.
    await page.locator("#hubQuery").waitFor({ timeout: 30000 });
    const dl = row("gemma-3-1b-pt").getByRole("button", { name: /^Download Gemma 3 1B PT, 806 MB$/ });
    if (!(await dl.isEnabled())) throw new Error("the PT download button is not enabled online");
    await shot(page, "course-models");
    await openModels("local");
    if (!(await page.locator("#modelList li", { hasText: "Gemma 3 1B Instruct" }).getByText("Course model").count())) throw new Error("no Course model tag in the local list");
  });

  if (!CDP)
    await step("course models: one click downloads, checks and installs (mock core)", async () => {
      const ui = new URL("../../desktop/ui/index.html?mock", import.meta.url).href;
      const p = await browser.newPage({ viewport: { width: W, height: H } });
      try {
        await p.goto(ui);
        await p.click("#openModels");
        await p.click("#tabHub");
        const row = p.locator('.course-row[data-course="gemma-3-1b-pt"]');
        await row.getByRole("button", { name: /Download Gemma 3 1B PT/ }).click();
        await row.getByText(/Downloading \d+%/).waitFor({ timeout: 5000 });
        if (!(await p.locator("#dlList li", { hasText: "gemma-3-1b-pt.Q4_K_M.gguf" }).count())) throw new Error("no row in Downloads");
        await shot(p, "course-downloading");
        await row.getByText("On this computer").waitFor({ timeout: 30000 });
        await p.click("#tabLocal");
        await p.locator("#modelList li", { hasText: "Gemma 3 1B PT" }).getByText("Course model").waitFor({ timeout: 5000 });

        // A download started far down the dialog scrolls its progress bar into view.
        await p.click("#tabHub");
        await p.fill("#hubQuery", "qwen");
        await p.keyboard.press("Enter");
        await p.locator(".hit").first().click({ timeout: 10000 });
        await p.locator(".files tbody tr").first().waitFor({ timeout: 10000 });
        const body = p.locator("#modelsDlg .dlg-body");
        await body.evaluate((el) => (el.scrollTop = el.scrollHeight));
        await p.getByRole("button", { name: /^Download Q8_0/ }).click();
        const dlRow = p.locator("#dlList li", { hasText: "Qwen_Qwen3-0.6B-Q8_0.gguf" });
        await dlRow.waitFor({ timeout: 5000 });
        await p.waitForTimeout(800); // the smooth scroll
        const inView = await dlRow.evaluate((li) => {
          const r = li.getBoundingClientRect();
          const b = li.closest(".dlg-body").getBoundingClientRect();
          return r.top >= b.top && r.bottom <= b.bottom;
        });
        if (!inView) throw new Error("the new download's progress bar is scrolled out of view");
        await shot(p, "download-revealed");
      } finally {
        await p.close();
      }
    });

  if (!CDP)
    await step("course models: all three in a row, while the others download (mock core)", async () => {
      const ui = new URL("../../desktop/ui/index.html?mock&scenario=empty", import.meta.url).href;
      const p = await browser.newPage({ viewport: { width: W, height: H } });
      try {
        await p.goto(ui);
        await p.getByText("Download the course models").click();
        const btn = (id) => p.locator(`.course-row[data-course="${id}"] button`, { hasText: "Download" });
        // While the internet check runs, nothing can start a download that would only wait.
        if (await btn("qwen3-0.6b").isEnabled()) throw new Error("Download enabled before the internet check answered");
        await p.waitForFunction(() => !document.querySelector('.course-row[data-course="qwen3-0.6b"] button').disabled, null, { timeout: 5000 });
        // A running download must not rebuild the other rows under the pointer.
        await btn("gemma-3-1b-it").click();
        const qwen = await btn("qwen3-0.6b").elementHandle();
        await p.waitForTimeout(800); // three download ticks
        if (!(await qwen.evaluate((el) => el.isConnected))) throw new Error("the Qwen3 row was rebuilt during the Gemma download");
        await qwen.click();
        await btn("gemma-3-1b-pt").click();
        for (const f of ["gemma-3-1b-it-Q4_K_M.gguf", "Qwen_Qwen3-0.6B-Q4_K_M.gguf", "gemma-3-1b-pt.Q4_K_M.gguf"])
          await p.locator("#dlList li", { hasText: f }).waitFor({ timeout: 5000 });
        await shot(p, "course-three-downloads");
        // The Hugging Face file table is not rebuilt under the pointer either.
        await p.fill("#hubQuery", "qwen");
        await p.keyboard.press("Enter");
        await p.locator(".hit").first().click({ timeout: 10000 });
        const q8 = await p.getByRole("button", { name: /^Download Q8_0/ }).elementHandle({ timeout: 10000 });
        await p.waitForTimeout(800); // three download ticks
        if (!(await q8.evaluate((el) => el.isConnected))) throw new Error("the file table was rebuilt during a download");
      } finally {
        await p.close();
      }
    });

  if (!CDP)
    await step("course models: every row says what is wrong and offers a way out (mock core)", async () => {
      const ui = new URL("../../desktop/ui/index.html?mock&scenario=coursestates", import.meta.url).href;
      const p = await browser.newPage({ viewport: { width: W, height: H } });
      try {
        await p.goto(ui);
        await p.click("#openModels");
        // The local list: honest tags, never "Included with Trurl".
        await p.locator("#modelList li", { hasText: "Gemma 3 4B Instruct" }).getByText("Checked by Trurl").waitFor({ timeout: 5000 });
        if (await p.getByText("Included with Trurl").count()) throw new Error("Included with Trurl is still shown");
        await p.click("#tabHub");
        const row = (id) => p.locator(`.course-row[data-course="${id}"]`);
        await row("qwen3-0.6b").waitFor({ timeout: 5000 });
        // Try again waits for the connection check, like Download.
        if (await row("gemma-3-1b-pt").getByRole("button", { name: /Try downloading/ }).isEnabled()) throw new Error("Try again enabled before the internet check answered");
        await p.locator("#hubQuery").waitFor({ timeout: 5000 });
        await row("gemma-3-1b-it").getByText("Damaged file").waitFor();
        await row("gemma-3-1b-it").getByRole("button", { name: /Delete the damaged Gemma 3 1B Instruct/ }).waitFor();
        await row("gemma-3-1b-pt").getByText("Download failed").waitFor();
        await row("gemma-3-1b-pt").getByText("The download stopped.").waitFor();
        await row("qwen3-0.6b").getByText("Another file has this name").waitFor();
        if (await row("qwen3-0.6b").getByRole("button").count()) throw new Error("a look-alike row offers a button");
        await shot(p, "course-states");
        // Try again restarts the failed download.
        await row("gemma-3-1b-pt").getByRole("button", { name: /Try downloading Gemma 3 1B PT again/ }).click();
        await row("gemma-3-1b-pt").getByText(/Downloading \d+%|Waiting to retry/).waitFor({ timeout: 5000 });
        // Delete the damaged file: the row offers Download again.
        await row("gemma-3-1b-it").getByRole("button", { name: /Delete the damaged/ }).click();
        await p.locator("#confirmOk").click();
        await row("gemma-3-1b-it").getByRole("button", { name: /Download Gemma 3 1B Instruct/ }).waitFor({ timeout: 5000 });
      } finally {
        await p.close();
      }
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
    // This test computer has AVX2: Q4_K_M, and no claim about Q4_0 speed.
    const legend = await page.locator(".quant-legend").innerText();
    if (!legend.includes("The recommended one, Q4_K_M, is listed first.")) throw new Error(`legend: ${legend}`);
    const first = await page.locator(".files tbody tr").first().innerText();
    if (!/Q4_K_M[\s\S]*Recommended/.test(first)) throw new Error(`first file row: ${first}`);
    await closeDialogs();
  });

  if (!CDP)
    await step("Hugging Face on a 2008 laptop recommends Q4_0 and says why (mock core)", async () => {
      // The real core picks by this computer's processor; the mock plays an SSSE3 one.
      const ui = new URL("../../desktop/ui/index.html?mock&scenario=oldcpu", import.meta.url).href;
      const p = await browser.newPage({ viewport: { width: W, height: H } });
      try {
        await p.goto(ui);
        await p.click("#openModels");
        await p.click("#tabHub");
        await p.fill("#hubQuery", "qwen");
        await p.keyboard.press("Enter");
        await p.locator(".hit").first().click({ timeout: 10000 });
        await p.locator(".files tbody tr").first().waitFor({ timeout: 10000 });
        const legend = await p.locator(".quant-legend").innerText();
        if (!legend.includes("On this computer's processor, Q4_0 files answer fastest")) throw new Error(`legend: ${legend}`);
        const first = await p.locator(".files tbody tr").first().innerText();
        if (!/Q4_0[\s\S]*Recommended/.test(first)) throw new Error(`first file row: ${first}`);
        await shot(p, "hf-details-oldcpu");
      } finally {
        await p.close();
      }
    });

  await step("chats: every chat is saved, opens with its settings and carries on, then deletes", async () => {
    await closeDialogs();
    await page.click("#openChats");
    await page.locator(".chat-row").first().waitFor({ timeout: 10000 });
    // The chats of the steps above, newest first; the pirate one among them.
    const pirate = page.locator(".chat-row", { hasText: "Hello! Who are you?" }).first();
    await pirate.waitFor({ timeout: 5000 });
    await shot(page, "chats");
    await pirate.getByRole("button", { name: /^Open the chat/ }).click();
    await page.locator("#chatsDlg").waitFor({ state: "hidden", timeout: 10000 });
    await waitPill("Ready", 300000);
    if (!/pirate/.test(await page.inputValue("#sysPrompt"))) throw new Error("the chat's system prompt did not come back");
    if ((await page.locator("#tempOut").textContent()) !== "0.0") throw new Error("the chat's temperature did not come back");
    if (!(await page.locator(".turn", { hasText: "Hello! Who are you?" }).count())) throw new Error("the chat's turns did not come back");
    // It carries on: the model sees the earlier turn and the system prompt.
    await ask("What did I just ask you? Answer in one short sentence.");
    // Written with the chat's system prompt and temperature (what the answer says it got).
    const meta = await lastTurn().innerText();
    if (!/Temperature 0\.0/.test(meta) || !/System prompt: .You are a friendly pirate/.test(meta)) throw new Error("the new answer was not written with the chat's settings: " + meta.slice(0, 200));
    await page.click("#openChats");
    const row = page.locator(".chat-row.current");
    if (!/2 questions/.test(await row.innerText())) throw new Error("the new answer was not saved into the same chat: " + (await row.innerText()));
    // Delete it: the screen clears.
    await row.getByRole("button", { name: /^Delete the chat/ }).click();
    await page.locator("#confirmOk").click();
    await page.waitForFunction(() => !document.querySelector(".turn"), null, { timeout: 10000 });
    // Delete all: the list is empty.
    await page.click("#deleteAllChats");
    await page.locator("#confirmOk").click();
    await page.getByText("No saved chats yet.").waitFor({ timeout: 10000 });
    await closeDialogs();
    // Leave the settings as the next steps expect them.
    if (await page.locator("#sysClear").isVisible()) await page.click("#sysClear");
    if (await page.locator("#tempReset").isVisible()) await page.click("#tempReset");
  });

  await step("about and licenses", async () => {
    await page.click("#openAbout");
    await page.locator(".licenses details").first().waitFor({ timeout: 10000 });
    const text = await page.locator("#aboutBody").innerText();
    if (!/Gemma Terms of Use/.test(text) || !/By Julien Barbier, for Cyberiad Labs/.test(text)) throw new Error("about is missing licenses or attribution");
    await shot(page, "about");
    await closeDialogs();
  });

  await step("automatic update check can be turned off, and stays off", async () => {
    await page.click("#openAbout");
    const box = page.locator("#autoUpdate");
    if (!(await box.isChecked())) throw new Error("automatic update check should start on");
    await box.uncheck();
    await closeDialogs();
    // The saved setting, read back from the core, not the checkbox left on screen.
    const saved = await page.evaluate(async () => (await window.trurl.call("snapshot")).settings.auto_update_check);
    if (saved !== false) throw new Error(`core still has auto_update_check=${saved}`);
    await page.click("#openAbout");
    if (await page.locator("#autoUpdate").isChecked()) throw new Error("About shows it on again after reopening");
    await page.locator("#autoUpdate").check(); // back on, for the steps after this one
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
  await step("Later hides the offer; Check for updates in About brings it back", async () => {
    await page.getByRole("button", { name: "Later" }).click();
    await page.getByText(`Update available: version ${expect}`).waitFor({ state: "detached", timeout: 5000 });
    await page.click("#openAbout");
    await page.getByRole("button", { name: "Check for updates" }).click();
    await page.getByText(`Version ${expect} is available.`).waitFor({ timeout: 60000 });
    await closeDialogs();
    await page.getByText(`Update available: version ${expect}`).waitFor({ timeout: 5000 });
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
    // The course list stays, with what is here; the missing one says how to get it.
    await page.locator(".course-row").nth(2).waitFor({ timeout: 5000 });
    if (await page.getByRole("button", { name: /Download Gemma 3 1B PT/ }).isEnabled()) throw new Error("download enabled offline");
    await page.getByText("Connect to the internet to download them, or import them from the USB stick").waitFor({ timeout: 5000 });
    await shot(page, "hf-offline");
    await closeDialogs();
    await ask("Say hello.");
    // Saved with no internet: the history never needs it.
    await page.click("#openChats");
    await page.locator(".chat-row", { hasText: "Say hello." }).waitFor({ timeout: 10000 });
    await closeDialogs();
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
