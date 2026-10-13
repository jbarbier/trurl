// Trurl desktop UI. Talks to the core only through window.trurl (api.js).
// Contract: contracts/events.md. Snapshot on start, then apply events.

(function () {
  "use strict";

  const api = window.trurl;
  const $ = (id) => document.getElementById(id);

  // ---------- Small helpers ----------------------------------------------

  // h("div", { class: "x", onclick: fn, "aria-label": "y" }, child, "text", ...)
  function h(tag, props, ...kids) {
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(props || {})) {
      if (v == null || v === false) continue;
      if (k === "class") el.className = v;
      else if (k === "text") el.textContent = v;
      else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
      else el.setAttribute(k, v === true ? "" : v);
    }
    put(el, kids, "append");
    return el;
  }
  // Appends (or replaces with) children, skipping the null/false/0/"" left by `cond && node`.
  function put(el, kids, how = "replaceChildren") {
    el[how](...[kids].flat(Infinity).filter((k) => k != null && k !== false && k !== 0 && k !== ""));
    return el;
  }
  const svg = (paths) => {
    const s = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    s.setAttribute("viewBox", "0 0 20 20");
    s.setAttribute("class", "ico");
    s.setAttribute("aria-hidden", "true");
    s.innerHTML = paths;
    return s;
  };
  const ICON = {
    chev: '<path d="m6 8 4 4 4-4"/>',
    back: '<path d="M16 10H4m5-5-5 5 5 5"/>',
    close: '<path d="m5 5 10 10M15 5 5 15"/>',
    usb: '<path d="M7 2h6v6H7zM5 8h10v6a4 4 0 0 1-4 4H9a4 4 0 0 1-4-4z"/>',
    download: '<path d="M10 3v10m-4-4 4 4 4-4M4 17h12"/>',
  };
  const chevron = () => {
    const s = svg(ICON.chev);
    s.classList.add("chev");
    return s;
  };
  const spinner = () => h("span", { class: "spin", "aria-hidden": "true" });
  const dots = () => h("span", { class: "dots", "aria-hidden": "true" }, h("i"), h("i"), h("i"));

  const nf = new Intl.NumberFormat("en-US");
  const fmt = {
    num: (n) => nf.format(n),
    size(b) {
      if (b >= 1e9) return (b / 1e9).toFixed(1) + " GB";
      if (b >= 1e6) return Math.round(b / 1e6) + " MB";
      if (b >= 1e3) return Math.round(b / 1e3) + " KB";
      return Math.round(b) + " B";
    },
    ram: (b) => Math.round(b / 1024 ** 3) + " GB",
    date: (iso) => (iso ? new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric" }) : null),
    params(n) {
      if (!n) return null;
      if (n >= 1e9) return (n / 1e9).toFixed(1).replace(/\.0$/, "") + "B parameters";
      if (n >= 1e6) return Math.round(n / 1e6) + "M parameters";
      return Math.max(1, Math.round(n / 1e3)) + "K parameters";
    },
    tokens: (n) => nf.format(n) + " tokens",
    thought(ms) {
      const s = Math.max(1, Math.round(ms / 1000));
      return s < 60 ? `Thought for ${s}s` : `Thought for ${Math.floor(s / 60)} min ${s % 60} s`;
    },
    pct: (done, total) => (total > 0 ? Math.min(100, Math.floor((done / total) * 100)) : 0),
  };
  const FIT = { green: "Fits easily", orange: "Tight fit", red: "Too big" };
  const fitEl = (fit) => h("span", { class: "fit fit-" + fit }, FIT[fit] || "Fit unknown");
  const bar = (pct, tone) => {
    const b = h("div", { class: "bar" + (tone ? " " + tone : ""), role: "progressbar", "aria-valuemin": "0", "aria-valuemax": "100", "aria-valuenow": String(pct) }, h("i"));
    b.style.setProperty("--p", pct / 100);
    return b;
  };
  const basename = (p) => String(p).split(/[\\/]/).pop();

  // Keeps keyboard focus on "the same" control when a list is rebuilt.
  function keepFocus(container, rebuild) {
    const a = document.activeElement;
    const key = a && container.contains(a) ? a.dataset.key : null;
    rebuild();
    if (key) {
      const again = container.querySelector(`[data-key="${CSS.escape(key)}"]`);
      if (again && !again.disabled) again.focus();
    }
  }

  let liveTimer = 0;
  function announce(text) {
    const live = $("live");
    live.textContent = "";
    clearTimeout(liveTimer);
    liveTimer = setTimeout(() => (live.textContent = text), 60);
  }

  // ---------- Markdown ----------------------------------------------------

  // Links never navigate the app: they become plain text with the URL shown next to it.
  DOMPurify.addHook("afterSanitizeAttributes", (node) => {
    if (node.tagName !== "A") return;
    const href = node.getAttribute("href") || "";
    node.removeAttribute("href");
    node.removeAttribute("target");
    node.setAttribute("class", "link");
    if (href && href !== node.textContent.trim() && !href.startsWith("#")) node.setAttribute("data-href", href);
  });
  const PURIFY = {
    FORBID_TAGS: ["img", "picture", "source", "video", "audio", "iframe", "style", "form", "input", "button", "svg", "math"],
    FORBID_ATTR: ["style", "id", "class"],
  };
  const md = (text) => DOMPurify.sanitize(marked.parse(plainMath(text || ""), { gfm: true, breaks: true, async: false }), PURIFY);

  // ---------- State -------------------------------------------------------

  const S = {
    version: "",
    llama_build: "",
    models_dir: "",
    hardware: null,
    settings: null,
    models: [],
    engine: { state: "idle" },
    generating: false,
    downloads: [],
    course_models: [], // [{ id, name, note, file, size, fit }], fixed per Trurl version
    chats: [], // saved chats, newest first: [{ id, title, updated, turns, model }]
    chat_id: null, // the chat on screen (null: a new chat, not saved yet)
    importing: null, // { file, done, total }
    verifying: null, // { file, done, total }
    update: null, // { info, phase: "offer"|"installing"|"failed", done, total, error, hidden }
  };
  const views = new Map(); // turn id -> view

  const currentModel = () => S.models.find((m) => m.file === (S.engine.file || (S.settings && S.settings.model)));

  async function call(cmd, args) {
    try {
      return await api.call(cmd, args);
    } catch (e) {
      notice(e);
      throw e;
    }
  }
  const run = (cmd, args) => call(cmd, args).catch(() => null);

  // ---------- Status: header pill, composer, sidebar card ------------------

  function status() {
    const e = S.engine;
    if (!S.models.length) return { s: "idle", text: "No model yet" };
    if (e.state === "loading") return { s: "loading", text: `Loading ${e.name}…` };
    if (e.state === "failed") return { s: "failed", text: "Failed" };
    if (e.state !== "ready") return { s: "idle", text: "No model loaded" };
    if (S.generating) {
      const live = [...views.values()].find((v) => v.turn.status === "streaming");
      if (live && live.turn.answer) return { s: "answering", text: "Answering" };
      // Only a reasoning model thinks; the others are just reading the question.
      if (live && live.turn.reasoning) return { s: "thinking", text: "Thinking" };
      return { s: "starting", text: "Starting" };
    }
    return { s: "ready", text: "Ready" };
  }

  let lastStatus = "";
  function renderStatus() {
    const st = status();
    const pill = $("pill");
    pill.dataset.s = st.s;
    // Moving indicators only while the learner is waiting for something to appear.
    const indicator = st.s === "loading" || st.s === "starting" ? spinner() : st.s === "thinking" ? dots() : h("span", { class: "dot", "aria-hidden": "true" });
    pill.replaceChildren(indicator, h("span", { text: st.text }));
    const m = currentModel();
    $("chatTitle").textContent = S.engine.name || (m && m.name) || "Trurl";

    const key = st.s + "|" + (S.engine.name || "");
    if (key !== lastStatus) {
      lastStatus = key;
      const name = S.engine.name || "The model";
      const words = { ready: `${name} is ready.`, loading: `Loading ${name}.`, failed: `${name} did not load.`, thinking: "Thinking.", answering: "Answering." };
      if (words[st.s]) announce(words[st.s]);
    }
    renderComposer();
    renderModelCard();
    renderEngineBanner();
    // No model at all: the welcome screen holds the two actions, an idle message box would only confuse.
    $("composer").hidden = !S.models.length;
  }

  // A message sent while the model loads waits here and goes out when it is ready.
  let queued = false;

  function sendBlocker() {
    if (!S.models.length) return "Add a model to start chatting.";
    if (S.engine.state === "loading") return null;
    if (S.engine.state === "failed") return "The model did not load. Try again, or choose another model.";
    if (S.engine.state !== "ready") return "Choose a model to start chatting.";
    return null;
  }

  function renderComposer() {
    const btn = $("send");
    const note = $("composerNote");
    if (S.generating) {
      btn.classList.add("stop");
      btn.disabled = false;
      btn.replaceChildren(h("span", { class: "sq", "aria-hidden": "true" }), "Stop");
      btn.setAttribute("aria-label", "Stop the answer (Esc)");
      const st = status().s;
      note.textContent =
        st === "thinking"
          ? "The model is thinking. Press Esc or Stop to end it early."
          : st === "starting"
            ? "The model is reading your message. Press Esc or Stop to cancel."
            : "Writing an answer. Press Esc or Stop to end it early.";
      delete note.dataset.warn;
      return;
    }
    btn.classList.remove("stop");
    btn.textContent = "Send";
    btn.removeAttribute("aria-label");
    if (S.engine.state === "loading") {
      btn.disabled = queued;
      note.textContent = queued ? `Your message is sent when ${S.engine.name} is ready.` : `${S.engine.name} is loading. You can send now: your message waits until it is ready.`;
      delete note.dataset.warn;
      return;
    }
    const blocked = sendBlocker();
    // Disabled only for a real reason (shown next to it), never just because the box is empty.
    btn.disabled = !!blocked;
    note.textContent = blocked || "Press Enter to send. Shift+Enter starts a new line.";
    if (blocked) note.dataset.warn = "";
    else delete note.dataset.warn;
  }

  function renderModelCard() {
    const card = $("modelCard");
    const m = currentModel();
    if (!S.models.length) {
      card.replaceChildren(
        h("span", { class: "mc-row" }, h("span", { class: "mc-name", text: "No model yet" }), h("span", { class: "mc-change", text: "Add one" }))
      );
      return;
    }
    if (!m) {
      card.replaceChildren(h("span", { class: "mc-name", text: "No model chosen" }), h("span", { class: "mc-change", text: "Choose a model" }));
      return;
    }
    const e = S.engine;
    const engine =
      e.state === "loading" ? [spinner(), "Loading"] : e.state === "ready" ? [] : e.state === "failed" ? ["Did not load"] : ["Not loaded"];
    card.replaceChildren(
      h("span", { class: "mc-name", text: m.name }),
      h("span", { class: "mc-row" }, h("span", { class: "mc-meta", text: [m.quant, fmt.size(m.size)].filter(Boolean).join(" · ") }), h("span", { class: "mc-change", text: "Change" })),
      h("span", { class: "mc-row" }, fitEl(m.fit), h("span", { class: "mc-engine" }, ...engine))
    );
    card.setAttribute("aria-label", `Model: ${m.name}, ${FIT[m.fit] || ""}${engine.length ? ", " + engine[engine.length - 1] : ""}. Change model.`);
  }

  // ---------- Banners: engine failed, update card, notices -----------------

  const slots = { engine: h("div"), update: h("div"), notices: h("div") };
  $("banners").append(slots.engine, slots.update, slots.notices);

  function banner(tone, title, hint, actions, onDismiss) {
    return h(
      "div",
      { class: "banner" + (tone ? " banner-" + tone : ""), role: tone === "take" ? "alert" : "status" },
      h("div", { class: "banner-body" }, h("p", { class: "banner-title", text: title }), hint && h("p", { class: "banner-hint", text: hint }), actions && h("div", { class: "banner-actions" }, actions)),
      onDismiss && h("button", { class: "icon-btn", type: "button", "aria-label": "Dismiss", onclick: onDismiss }, svg(ICON.close))
    );
  }

  function renderEngineBanner() {
    const e = S.engine;
    if (e.state !== "failed") return slots.engine.replaceChildren();
    if (slots.engine.dataset.for === e.file && slots.engine.firstChild) return;
    slots.engine.dataset.for = e.file;
    const err = e.error || {};
    slots.engine.replaceChildren(
      banner("take", err.title || `${e.name} did not load.`, err.hint, [
        h("button", { class: "btn btn-primary", type: "button", onclick: () => run("reload") }, "Try again"),
        h("button", { class: "btn btn-secondary", type: "button", onclick: () => openModels("local") }, "Choose another model"),
      ])
    );
  }

  function notice(err) {
    if (!err) return;
    if (!err.title) {
      err = { code: "internal", title: "Something went wrong inside Trurl.", hint: "Try again. If it keeps happening, restart Trurl.", detail: String((err && (err.detail || err.message)) || err) };
    }
    const shown = [...slots.notices.children].some((n) => n.dataset.title === err.title);
    if (shown) return;
    const el = banner(err.code === "offline" ? "rev" : "take", err.title, err.hint, null, () => {
      el.remove();
      $("input").focus();
    });
    el.dataset.title = err.title;
    slots.notices.append(el);
    while (slots.notices.children.length > 3) slots.notices.firstChild.remove();
  }

  function renderUpdate() {
    const u = S.update;
    if (!u || u.hidden) return slots.update.replaceChildren();
    keepFocus(slots.update, () => {
      let body;
      if (u.phase === "installing") {
        const pct = fmt.pct(u.done, u.total);
        body = h(
          "div",
          { class: "progress" },
          h("div", { class: "progress-top" }, h("span", { text: "Downloading the update. Trurl restarts when it is done." }), h("span", { class: "num", text: pct + "%" })),
          bar(pct)
        );
      } else {
        const failed = u.phase === "failed";
        const blocked = S.generating;
        body = [
          failed && h("p", { class: "banner-hint", text: `${u.error.title} ${u.error.hint || ""}`.trim() }),
          h(
            "div",
            { class: "banner-actions" },
            h("button", { class: "btn btn-primary", type: "button", "data-key": "upd-now", disabled: blocked, "aria-describedby": blocked ? "updReason" : null, onclick: installUpdate }, failed ? "Try again" : "Update now"),
            h("button", { class: "btn btn-secondary", type: "button", "data-key": "upd-later", onclick: () => ((u.hidden = true), renderUpdate(), $("input").focus()) }, "Later"),
            blocked && h("span", { class: "banner-reason", id: "updReason", text: "You can update when the answer is finished." })
          ),
        ];
      }
      slots.update.replaceChildren(
        h(
          "div",
          { class: "banner", role: "status" },
          h(
            "div",
            { class: "banner-body" },
            h("p", { class: "banner-title" }, "Update available: version ", h("span", { class: "num", text: u.info.version })),
            u.info.notes && h("p", { class: "banner-notes", text: u.info.notes }),
            body
          )
        )
      );
    });
  }

  function offerUpdate(info) {
    if (S.update && S.update.info.version === info.version && S.update.phase !== "failed") {
      S.update.hidden = false;
    } else S.update = { info, phase: "offer", done: 0, total: 0, hidden: false };
    renderUpdate();
  }

  function installUpdate() {
    if (S.generating || S.update.phase === "installing") return;
    Object.assign(S.update, { phase: "installing", done: 0, total: 0 });
    renderUpdate();
    api.call("update_install").catch((e) => {
      Object.assign(S.update, { phase: "failed", error: e });
      renderUpdate();
    });
  }

  // ---------- Sidebar controls -------------------------------------------

  const sys = $("sysPrompt");
  const temp = $("temp");
  const ctx = $("ctx");
  let sysTimer = 0;

  function saveSystemPrompt() {
    clearTimeout(sysTimer);
    if (!S.settings || sys.value === S.settings.system_prompt) return;
    S.settings.system_prompt = sys.value;
    run("set_system_prompt", { text: sys.value }).then(() => {
      const saved = $("sysSaved");
      saved.hidden = false;
      clearTimeout(saved._t);
      saved._t = setTimeout(() => (saved.hidden = true), 2000);
    });
  }
  // Grows with the text, up to the CSS max-height, so a long prompt stays readable.
  function fitSys() {
    sys.style.height = "auto";
    sys.style.height = sys.scrollHeight + 2 + "px";
  }
  sys.addEventListener("input", () => {
    fitSys();
    $("sysSaved").hidden = true;
    $("sysClear").hidden = !sys.value;
    clearTimeout(sysTimer);
    sysTimer = setTimeout(saveSystemPrompt, 800);
  });
  sys.addEventListener("blur", saveSystemPrompt);
  $("sysClear").addEventListener("click", () => {
    sys.value = "";
    $("sysClear").hidden = true;
    saveSystemPrompt();
    sys.focus();
  });

  function showTemp(v) {
    $("tempOut").textContent = Number(v).toFixed(1);
    temp.style.setProperty("--p", String(Number(v) / 2));
    $("tempReset").hidden = !S.settings || Math.abs(v - S.settings.default_temperature) < 0.05;
  }
  temp.addEventListener("input", () => showTemp(Number(temp.value)));
  temp.addEventListener("change", () => run("set_temperature", { value: Math.round(Number(temp.value) * 10) / 10 }));
  $("tempReset").addEventListener("click", () => {
    temp.value = S.settings.default_temperature;
    showTemp(S.settings.default_temperature);
    run("set_temperature", { value: S.settings.default_temperature });
    temp.focus();
  });

  ctx.addEventListener("change", () => run("set_context", { value: Number(ctx.value) }));
  $("ctxReset").addEventListener("click", () => {
    run("set_context", { value: S.settings.default_context });
    ctx.focus();
  });

  function applySettings() {
    const st = S.settings;
    if (!st) return;
    for (const v of views.values()) {
      if (v.end.querySelector("[data-bigger]")) v.end.replaceChildren(...ending(v.turn, v), ...copyButton(v));
    }
    if (document.activeElement !== sys) {
      sys.value = st.system_prompt || "";
      $("sysClear").hidden = !sys.value;
      fitSys();
    }
    if (document.activeElement !== temp || !temp.matches(":active")) temp.value = st.temperature;
    showTemp(st.temperature);
    const choices = st.context_choices || [];
    if (ctx.options.length !== choices.length || [...ctx.options].some((o, i) => Number(o.value) !== choices[i])) {
      // The unit is in the label ("Context size, tokens"), so "2,048 (default)" fits a narrow sidebar.
      ctx.replaceChildren(...choices.map((c) => h("option", { value: String(c), text: fmt.num(c) + (c === st.default_context ? " (default)" : "") })));
    }
    ctx.value = String(st.context_size);
    $("ctxReset").hidden = st.context_size === st.default_context;
  }

  // ---------- Transcript --------------------------------------------------

  const scroller = $("transcript");
  const turnsEl = $("turns");
  // Follow new text only if the learner was already at the bottom; read the position
  // just before the change so a scroll that is still in flight is never undone.
  const atEnd = () => scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 48;
  function follow(change) {
    const was = atEnd();
    change();
    if (was) scrollToEnd();
  }

  scroller.addEventListener("scroll", () => ($("jump").hidden = atEnd()), { passive: true });
  $("jump").addEventListener("click", () => {
    scrollToEnd();
    $("input").focus();
  });
  function scrollToEnd() {
    scroller.scrollTop = scroller.scrollHeight;
    $("jump").hidden = true;
  }

  // Streaming text is painted at most every 100 ms, whatever the token rate.
  const dirty = new Set();
  let paintTimer = 0;
  function schedule(view) {
    dirty.add(view);
    if (!paintTimer) paintTimer = setTimeout(flush, 100);
  }
  function flush() {
    paintTimer = 0;
    follow(() => dirty.forEach(paint));
    dirty.clear();
  }

  function makeView(turn) {
    const ans = h("div", { class: "md" });
    const end = h("div", { class: "end" });
    const answer = h("div", { class: "answer" }, h("div", { class: "who", text: turn.model || "" }), turnSettings(turn), ans, end);
    const el = h("article", { class: "turn", "aria-label": "Question and answer" }, h("div", { class: "msg-user", text: turn.user }), answer);
    const view = { turn: { ...turn }, el, answer, ans, end, think: null, painted: { reasoning: null, answer: null, status: null, live: null } };
    views.set(turn.id, view);
    paint(view);
    return view;
  }

  // What this answer was written with: comparing two answers is the lesson.
  function turnSettings(t) {
    if (t.temperature == null) return null;
    const sp = (t.system_prompt || "").trim();
    const short = sp.length > 240 ? sp.slice(0, 238) + "…" : sp;
    const text = `Temperature ${Number(t.temperature).toFixed(1)} · ` + (sp ? `System prompt: “${short}”` : "No system prompt");
    return h("div", { class: "turn-settings", title: sp || null, text });
  }

  function ensureThink(view) {
    if (view.think) return view.think;
    const id = "think-" + view.turn.id;
    const label = h("span", { class: "think-label" });
    const ind = h("span", { class: "think-ind" });
    const toggle = h("button", { class: "think-toggle", type: "button", "aria-expanded": "true", "aria-controls": id }, ind, label, chevron());
    const body = h("div", { class: "think-body", id });
    const box = h("div", { class: "think" }, toggle, body);
    toggle.addEventListener("click", () => setExpanded(view, toggle.getAttribute("aria-expanded") !== "true"));
    view.answer.insertBefore(box, view.ans);
    view.think = { box, toggle, label, ind, body };
    return view.think;
  }
  function setExpanded(view, open) {
    view.think.toggle.setAttribute("aria-expanded", String(open));
    view.think.body.hidden = !open;
  }

  function paint(view) {
    const t = view.turn;
    const p = view.painted;
    const streaming = t.status === "streaming";

    if (t.reasoning) {
      const th = ensureThink(view);
      if (p.reasoning !== t.reasoning) th.body.textContent = p.reasoning = t.reasoning;
      const live = streaming && !t.answer && t.thought_ms == null;
      const label = live
        ? "Thinking…"
        : t.thought_ms != null
          ? fmt.thought(t.thought_ms)
          : t.status === "stopped" && !t.answer
            ? "Stopped while thinking"
            : t.status === "cut" && !t.answer
              ? "Ran out of room while thinking"
              : "Thought";
      th.label.textContent = label;
      if (p.live !== live) {
        // Open while thinking, closed once the answer starts. The learner can toggle any time.
        setExpanded(view, live);
        th.ind.replaceChildren(live ? dots() : "");
        th.box.toggleAttribute("data-live", live);
        p.live = live;
      }
    }

    if (p.answer !== t.answer) {
      p.answer = t.answer;
      view.ans.innerHTML = md(t.answer);
    }
    const waiting = streaming && !t.answer && !t.reasoning;
    if (waiting && !view.ans.firstChild) view.ans.append(h("span", { class: "who" }, dots()));
    view.ans.hidden = !t.answer && !waiting;

    if (p.status !== t.status) {
      p.status = t.status;
      view.end.replaceChildren(...ending(t, view), ...copyButton(view));
      // Ran out of room while thinking: the reasoning is all there is to read.
      if (view.think && !t.answer && (t.status === "cut" || (t.error && t.error.code === "context_full"))) setExpanded(view, true);
    }
  }

  // Learners copy answers into their notes (course 1.4.4): one click, the text as written.
  function copyButton(view) {
    const t = view.turn;
    if (t.status === "streaming" || !t.answer) return [];
    const btn = h("button", { class: "link-btn copy-btn", type: "button", "aria-label": "Copy this answer" }, "Copy");
    btn.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(view.ans.innerText.trim());
        btn.textContent = "Copied";
      } catch {
        // No clipboard access: select the answer so Ctrl+C copies it.
        const r = document.createRange();
        r.selectNodeContents(view.ans);
        const sel = getSelection();
        sel.removeAllRanges();
        sel.addRange(r);
        btn.textContent = "Selected: press Ctrl+C";
      }
      setTimeout(() => (btn.textContent = "Copy"), 2500);
    });
    return [btn];
  }

  // The next context size above both the one this turn ran in and the current one.
  function nextContext(t) {
    const choices = (S.settings && S.settings.context_choices) || [];
    const now = Math.max(t.context || 0, (S.settings && S.settings.context_size) || 0);
    return choices.find((c) => c > now);
  }

  function ending(t, view) {
    if (t.status === "stopped") return [h("span", { class: "end-note", text: "Stopped" })];
    if (t.status === "cut" || t.status === "error") {
      const err = t.error || {};
      const cut = t.status === "cut" || err.code === "context_full";
      const title = err.title || (cut ? "The conversation ran out of room." : "Something went wrong with this answer.");
      const hint = err.hint || (cut ? "Start a new chat or increase the context size." : "Try again.");
      const acts = [];
      if (cut) {
        // One click to the next context size, with the question back in the box.
        const next = nextContext(t);
        if (next)
          acts.push(
            h(
              "button",
              {
                class: "btn btn-primary btn-small",
                type: "button",
                "data-bigger": "",
                onclick: async () => {
                  // Worked out again on click: the context may have changed since.
                  const n = nextContext(t);
                  if (!n || S.generating) return;
                  await run("set_context", { value: n });
                  input.value = t.user;
                  input.dispatchEvent(new Event("input"));
                  send(); // waits for the model to load, then asks
                },
              },
              `Use ${fmt.num(next)} tokens and ask again`
            )
          );
        // "New chat" helps only when earlier messages fill the context.
        // The core says "Start a new chat" only when earlier answers fill the context.
        if (/new chat/i.test(err.hint || "")) acts.push(h("button", { class: "btn btn-secondary btn-small", type: "button", onclick: newChat }, "New chat"));
        if (!t.answer && t.reasoning)
          acts.push(h("p", { class: "h think-tip", text: "Small thinking models sometimes think for a very long time: that is normal. To skip the thinking, start your message with /no_think." }));
      }
      let detail = null;
      if (err.detail) {
        const id = "detail-" + t.id;
        detail = h("pre", { class: "detail", id, hidden: true, text: err.detail });
        const btn = h("button", { class: "link-btn", type: "button", "aria-expanded": "false", "aria-controls": id }, "Details");
        btn.addEventListener("click", () => {
          const open = detail.hidden;
          detail.hidden = !open;
          btn.setAttribute("aria-expanded", String(open));
        });
        acts.push(btn);
      }
      return [
        h(
          "div",
          { class: "inline-note" + (cut ? "" : " take"), role: "status" },
          h("p", { class: "t", text: title }),
          h("p", { class: "h", text: hint }),
          acts.length && h("div", { class: "acts" }, acts),
          detail
        ),
      ];
    }
    return [];
  }

  function renderEmpty() {
    if (views.size) return;
    if (!S.models.length) {
      turnsEl.replaceChildren(
        h(
          "div",
          { class: "hello" },
          h("h2", { text: "No model yet" }),
          h("p", { text: "A model is the file that reads your question and writes the answer. Add one to start." }),
          h(
            "div",
            { class: "big-actions" },
            h("button", { class: "big-action", type: "button", "data-import": "" }, svg(ICON.usb), h("strong", { text: "Import a model file" }), h("span", { text: "From a USB stick or a folder on this computer." })),
            h("button", { class: "big-action", type: "button", onclick: () => openModels("hub") }, svg(ICON.download), h("strong", { text: "Download the course models" }), h("span", { text: "Or any model on Hugging Face. Needs internet once." }))
          ),
          h("div", { class: "import-slot" }),
          h("p", { class: "small", text: "Once a model is on this computer, it works without internet." })
        )
      );
      renderImport();
      return;
    }
    const m = currentModel();
    const e = S.engine;
    if (e.state === "failed") {
      // The red banner above says what went wrong and offers the two ways out.
      turnsEl.replaceChildren(h("div", { class: "hello" }, h("h2", { text: "The model is not running" }), h("p", { text: "Try again, or choose another model. Models with a green dot fit this computer best." })));
      return;
    }
    if (e.state === "idle") {
      turnsEl.replaceChildren(
        h("div", { class: "hello" }, h("h2", { text: "Choose a model" }), h("p", { text: "Pick which model answers your questions." }), h("div", { class: "examples" }, h("button", { class: "btn btn-primary", type: "button", onclick: () => openModels("local") }, "Choose a model")))
      );
      return;
    }
    const thinker = S.models.find((x) => x.reasoning_hint && !x.broken);
    const examples = ["Where is Accra?", "What is 17 + 25?", "Show me Python code that counts to 3"];
    const loading = e.state === "loading";
    turnsEl.replaceChildren(
      h(
        "div",
        { class: "hello" },
        h("h2", { text: loading ? `Loading ${e.name}…` : "Ask your first question" }),
        loading
          ? h("p", { text: "This can take a minute on older computers. You can type your question now: it is sent as soon as the model is ready." })
          : h("p", { text: `${m ? m.name : "The model"} runs on this computer. What you type stays here, and no internet is needed.` }),
        h("div", { class: "examples" }, examples.map((q) => h("button", { class: "example", type: "button", onclick: () => askExample(q) }, q))),
        thinker && (!m || !m.reasoning_hint) && h("p", { class: "small", text: `Tip: switch to ${thinker.name} to watch a model think before it answers.` })
      )
    );
  }

  function askExample(q) {
    const input = $("input");
    input.value = q;
    autosize();
    renderComposer();
    if (!sendBlocker()) send();
    else input.focus();
  }

  function resetTranscript(turns) {
    views.clear();
    dirty.clear();
    turnsEl.replaceChildren();
    for (const t of turns) turnsEl.append(makeView(t).el);
    renderEmpty();
    scrollToEnd();
  }

  // ---------- Composer ----------------------------------------------------

  const input = $("input");
  function autosize() {
    input.style.height = "auto";
    input.style.height = Math.min(input.scrollHeight, 182) + "px";
  }
  input.addEventListener("input", () => {
    autosize();
    renderComposer();
  });
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      if (!S.generating) send();
    }
  });
  $("composer").addEventListener("submit", (e) => {
    e.preventDefault();
    if (S.generating) run("stop");
    else send();
  });

  async function send() {
    const text = input.value.trim();
    if (!text || sendBlocker() || S.generating) return;
    if (S.engine.state === "loading") {
      queued = true;
      renderComposer();
      return;
    }
    queued = false;
    try {
      await call("send", { text });
      input.value = "";
      autosize();
      renderComposer();
    } catch (_) {
      /* the notice already explains it; the text stays in the box */
    }
  }

  async function newChat() {
    if (S.generating) {
      const ok = await confirmBox({
        title: "Start a new chat?",
        text: "The model is still answering. The answer stops here and stays in your chats.",
        ok: "Start a new chat",
        keep: "Keep this answer",
      });
      if (!ok) return;
    }
    run("new_chat");
    input.focus();
  }
  $("newChat").addEventListener("click", newChat);

  // ---------- Dialogs -----------------------------------------------------

  const opener = new WeakMap();
  function openDialog(dlg) {
    if (dlg.open) return;
    opener.set(dlg, document.activeElement);
    dlg.showModal();
  }
  document.querySelectorAll("dialog").forEach((dlg) => {
    dlg.addEventListener("close", () => {
      const back = opener.get(dlg);
      if (back && back.isConnected) back.focus();
    });
    dlg.addEventListener("click", (e) => {
      if (e.target === dlg) dlg.close(); // click on the backdrop
    });
    dlg.addEventListener("keydown", (e) => {
      if (e.key !== "Tab") return;
      const f = [...dlg.querySelectorAll("button, input, select, textarea, summary, [tabindex]:not([tabindex='-1'])")].filter((x) => !x.disabled && x.getClientRects().length);
      if (!f.length) return;
      const first = f[0];
      const last = f[f.length - 1];
      if (e.shiftKey && document.activeElement === first) (last.focus(), e.preventDefault());
      else if (!e.shiftKey && document.activeElement === last) (first.focus(), e.preventDefault());
    });
  });

  function confirmBox({ title, text, ok, keep = "Cancel", danger }) {
    const dlg = $("confirmDlg");
    $("confirmTitle").textContent = title;
    $("confirmText").textContent = text;
    $("confirmCancel").textContent = keep;
    const okBtn = $("confirmOk");
    okBtn.textContent = ok;
    okBtn.className = "btn " + (danger ? "btn-primary btn-danger" : "btn-primary");
    dlg.returnValue = "";
    openDialog(dlg);
    $("confirmCancel").focus();
    return new Promise((resolve) => dlg.addEventListener("close", () => resolve(dlg.returnValue === "ok"), { once: true }));
  }
  $("confirmOk").addEventListener("click", () => $("confirmDlg").close("ok"));
  $("confirmCancel").addEventListener("click", () => $("confirmDlg").close("cancel"));

  document.addEventListener("click", (e) => {
    const t = e.target.closest("[data-close], [data-import], [data-open-folder]");
    if (!t) return;
    if (t.hasAttribute("data-close")) t.closest("dialog").close();
    else if (t.hasAttribute("data-import")) importModel();
    else run("open_models_folder");
  });

  // ---------- Models: on this computer ------------------------------------

  async function useModel(m) {
    if (m.fit === "red") {
      const go = await confirmBox({
        title: `Load ${m.name}?`,
        text: "This model may be too big for this computer. It may be very slow or fail to load.",
        ok: "Load anyway",
      });
      if (!go) return;
    }
    if ($("modelsDlg").open) $("modelsDlg").close();
    run("select_model", { file: m.file });
  }

  async function deleteModel(m) {
    const go = await confirmBox({
      title: `Delete ${m.name}?`,
      text: `This frees ${fmt.size(m.size)} on this computer. ${m.course ? "To get it back, download it again under Hugging Face, For the course, or import it from the USB stick." : m.bundled ? "To get it back, import it from the USB stick or download it again." : "You can import or download it again later."}`,
      ok: "Delete",
      danger: true,
    });
    if (go) run("delete_model", { file: m.file });
  }

  function renderLocal() {
    $("modelsDir").textContent = S.models_dir;
    $("tabLocal").replaceChildren("On this computer", h("span", { class: "num", text: String(S.models.length) }));
    const list = $("modelList");
    const cur = currentModel();
    keepFocus(list, () => {
      if (!S.models.length) {
        list.replaceChildren(h("li", { class: "empty-note", text: "No models yet. Import a model file, or download one from Hugging Face." }));
        return;
      }
      list.replaceChildren(
        ...S.models.map((m) => {
          const inUse = cur && cur.file === m.file;
          const checking = S.verifying && S.verifying.file === m.file;
          return h(
            "li",
            { class: "row" + (inUse ? " current" : "") + (m.broken ? " broken" : "") },
            h(
              "div",
              { class: "row-main" },
              h("span", { class: "row-name", text: m.name }),
              h(
                "span",
                { class: "row-meta" },
                [m.quant, fmt.size(m.size)].filter(Boolean).join(" · "),
                fitEl(m.fit),
                m.broken && h("span", { class: "tag tag-take", text: "Damaged file" }),
                m.course ? h("span", { class: "tag tag-a", text: "Course model" }) : m.verified === true && h("span", { class: "tag tag-a", title: "The same file, byte for byte, as the one Trurl was tested with.", text: "Checked by Trurl" }),
                inUse && h("span", { class: "tag tag-held", text: "In use" }),
                checking && h("span", { class: "tag", text: `Checking the file ${fmt.pct(S.verifying.done, S.verifying.total)}%` })
              ),
              m.broken && h("span", { class: "row-hint", text: "This file is damaged. Delete it, then import or download it again." })
            ),
            h(
              "div",
              { class: "row-acts" },
              !inUse && !m.broken && h("button", { class: "btn btn-secondary btn-small", type: "button", "data-key": "use:" + m.file, "aria-label": `Use ${m.name}`, onclick: () => useModel(m) }, "Use"),
              h("button", { class: "btn btn-danger-quiet btn-small", type: "button", "data-key": "del:" + m.file, "aria-label": `Delete ${m.name}`, onclick: () => deleteModel(m) }, "Delete")
            )
          );
        })
      );
    });
    renderCourse();
  }

  async function importModel() {
    const path = await api.pickModelFile();
    if (!path) return;
    S.importing = { file: basename(path), done: 0, total: 0 };
    renderImport();
    api.call("import_model", { path }).catch((e) => {
      S.importing = null;
      renderImport();
      notice(e);
    });
  }
  function renderImport() {
    const im = S.importing;
    document.querySelectorAll(".import-slot").forEach((slot) => {
      if (!im) return slot.replaceChildren();
      const pct = fmt.pct(im.done, im.total);
      slot.replaceChildren(
        h(
          "div",
          { class: "progress" },
          h("div", { class: "progress-top" }, h("span", { text: `Copying ${im.file}` }), h("span", { class: "num", text: im.total ? `${pct}% · ${fmt.size(im.done)} of ${fmt.size(im.total)}` : "Starting…" })),
          bar(pct)
        )
      );
    });
  }

  // ---------- Downloads ----------------------------------------------------

  const DL_WORDS = {
    running: ["Downloading", ""],
    retrying: ["Waiting to retry", "rev"],
    paused: ["Paused", "ink"],
    verifying: ["Checking the file", ""],
    failed: ["Failed", "take"],
    done: ["Done", ""],
  };

  // The Downloads list sits at the top of the Models dialog. A learner who
  // clicked Download further down would not see it start, so scroll it into view.
  let reveal = null; // id of a download just started, until its row is on screen
  async function startDownload(cmd, args) {
    const id = await run(cmd, args);
    if (!id) return;
    reveal = id;
    renderDownloads();
  }
  async function resumeDownload(id) {
    await run("download_resume", { id });
    reveal = id;
    renderDownloads();
  }
  function revealDownload() {
    const li = reveal && $("dlList").querySelector(`[data-id="${CSS.escape(reveal)}"]`);
    if (!li) return;
    reveal = null;
    const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
    li.scrollIntoView({ block: "center", behavior: still ? "auto" : "smooth" });
  }

  function renderDownloads() {
    const list = $("dlList");
    $("downloads").hidden = !S.downloads.length;
    const seen = new Set();
    for (const d of S.downloads) {
      seen.add(d.id);
      let li = list.querySelector(`[data-id="${CSS.escape(d.id)}"]`);
      if (!li) {
        li = h("li", { class: "row dl", "data-id": d.id });
        li.ui = {
          name: h("span", { class: "dl-name", text: d.dest_file, title: `${d.repo}/${d.file}` }),
          status: h("span", { class: "dl-status" }),
          acts: h("span", { class: "row-acts" }),
          bar: bar(0),
          nums: h("div", { class: "dl-nums" }),
          hint: h("p", { class: "row-hint" }),
        };
        li.append(h("div", { class: "dl-top" }, li.ui.name, li.ui.status, li.ui.acts), li.ui.bar, li.ui.nums, li.ui.hint);
        list.append(li);
      }
      const [word, tone] = DL_WORDS[d.status] || [d.status, ""];
      const pct = fmt.pct(d.done, d.size);
      li.ui.status.textContent = word;
      li.ui.status.className = "dl-status " + tone;
      li.ui.bar.className = "bar" + (d.status === "failed" ? " take" : d.status === "retrying" || d.status === "paused" ? " rev" : "");
      li.ui.bar.style.setProperty("--p", pct / 100);
      li.ui.bar.setAttribute("aria-valuenow", String(pct));
      li.ui.bar.setAttribute("aria-label", `${d.dest_file}: ${pct}%`);
      li.ui.nums.replaceChildren(
        h("span", { class: "pct", text: pct + "%" }),
        h("span", { text: `${fmt.size(d.done)} of ${fmt.size(d.size)}` }),
        ...(d.status === "running" && d.rate > 0 ? [h("span", { text: fmt.size(d.rate) + "/s" })] : [])
      );
      const hint = d.status === "failed" && d.error ? `${d.error.title} ${d.error.hint || ""}`.trim() : d.status === "retrying" ? (d.error ? `${d.error.title} Trying again in a moment.` : "The connection dropped. Trurl tries again by itself.") : "";
      li.ui.hint.textContent = hint;
      li.ui.hint.hidden = !hint;
      li.ui.hint.style.color = d.status === "retrying" ? "var(--ink3)" : "";
      if (li.dataset.status !== d.status) {
        li.dataset.status = d.status;
        keepFocus(li.ui.acts, () => {
          const b = (label, cmd, key) => h("button", { class: "btn btn-secondary btn-small", type: "button", "data-key": key + d.id, onclick: () => run(cmd, { id: d.id }) }, label);
          li.ui.acts.replaceChildren(
            ...[
              (d.status === "running" || d.status === "retrying") && b("Pause", "download_pause", "p"),
              d.status === "paused" && b("Resume", "download_resume", "r"),
              d.status === "failed" && b("Try again", "download_resume", "r"),
              d.status !== "verifying" && d.status !== "done" && h("button", { class: "btn btn-quiet btn-small", type: "button", "data-key": "c" + d.id, "aria-label": `Cancel ${d.dest_file}`, onclick: async () => {
                    const ok = await confirmBox({ title: "Delete this download?", text: `${d.dest_file}: what was downloaded so far will be deleted. To stop for now and go on later, press Pause instead.`, ok: "Delete the download", keep: "Keep downloading", danger: true });
                    if (ok) run("download_cancel", { id: d.id });
                  } }, "Cancel"),
            ].filter(Boolean)
          );
        });
      }
    }
    list.querySelectorAll("li[data-id]").forEach((li) => {
      if (!seen.has(li.dataset.id)) li.remove();
    });
    const active = S.downloads.filter((d) => d.status !== "done" && d.status !== "failed");
    const chip = $("dlChip");
    chip.hidden = !active.length;
    if (active.length) {
      const done = active.reduce((a, d) => a + d.done, 0);
      const size = active.reduce((a, d) => a + d.size, 0);
      chip.textContent = `${active.length} download${active.length > 1 ? "s" : ""} · ${fmt.pct(done, size)}%`;
    }
    if (hub.details) renderFiles();
    renderCourse();
    revealDownload();
  }

  // ---------- Models: Hugging Face ---------------------------------------

  const hub = { online: null, query: "", results: null, searching: false, error: null, details: null, loading: null, lastHit: null };
  const hubEls = {};

  function buildHub() {
    const qInput = h("input", { type: "search", id: "hubQuery", placeholder: "For example: qwen3", spellcheck: "false" });
    const form = h(
      "form",
      { class: "search", role: "search" },
      h("label", { class: "sr-only", for: "hubQuery", text: "Search Hugging Face" }),
      qInput,
      h("button", { class: "btn btn-primary", type: "submit" }, "Search")
    );
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      search(qInput.value);
    });
    const tryLine = h(
      "p",
      { class: "try" },
      "Try:",
      ["qwen3", "gemma 3", "llama 3.2"].map((q) => h("button", { class: "link-btn", type: "button", onclick: () => ((qInput.value = q), search(q)) }, q))
    );
    Object.assign(hubEls, {
      course: h("section", { class: "course", "aria-labelledby": "courseTitle" }),
      status: h("div"),
      search: h("div", { class: "tabpanel" }, h("h3", { class: "sub-h", text: "Another model" }), form, tryLine),
      results: h("div", { class: "tabpanel" }),
      details: h("div", { class: "tabpanel" }),
      qInput,
    });
    $("panelHub").append(hubEls.course, hubEls.status, hubEls.search, hubEls.results, hubEls.details);
  }

  // The course models: the exact pinned files, one click each, first on the
  // download screen, so nobody picks a look-alike from a search.
  const courseModel = (c) => S.models.find((m) => m.file === c.file && m.course);
  // A different file under the course model's name (imported, or from another repo).
  const courseClash = (c) => !courseModel(c) && S.models.find((m) => m.file === c.file);
  // Its pinned download only: a search download of another commit can share the file name.
  const courseDownload = (c) => S.downloads.find((d) => d.id === c.download_id && d.status !== "done");
  // "In use" as in the terminal app: the model the engine runs or loads, not the saved choice.
  const runningFile = () => (S.engine.state === "ready" || S.engine.state === "loading" ? S.engine.file : null);

  // What a course row shows. Rows are rebuilt only when this changes, never on
  // every download tick (a rebuild under the pointer would swallow a click).
  function courseState(c) {
    const have = courseModel(c);
    if (have) return { kind: have.broken ? "broken" : have.file === runningFile() ? "inuse" : "here", have };
    if (courseClash(c)) return { kind: "clash" };
    const dl = courseDownload(c);
    if (dl) return { kind: dl.status === "failed" ? "failed" : dl.status === "paused" ? "paused" : "downloading", dl };
    return { kind: hub.online === true ? "missing" : hub.online === false ? "offline" : "checking" };
  }

  function courseRow(c, st) {
    const key = "c:" + c.id;
    const btn = (cls, label, onclick, extra = {}) => h("button", { class: "btn btn-small " + cls, type: "button", "data-key": key, onclick, ...extra }, label);
    let tag = null;
    let hint = null;
    let act = null;
    switch (st.kind) {
      case "inuse":
        tag = [h("span", { class: "tag tag-a", text: "On this computer" }), h("span", { class: "tag tag-held", text: "In use" })];
        break;
      case "here":
        tag = h("span", { class: "tag tag-a", text: "On this computer" });
        act = btn("btn-secondary", "Use", () => useModel(st.have), { "aria-label": `Use ${c.name}` });
        break;
      case "broken":
        tag = h("span", { class: "tag tag-take", text: "Damaged file" });
        hint = "This file is damaged. Delete it, then download it again.";
        act = btn("btn-danger-quiet", "Delete", () => deleteModel(st.have), { "aria-label": `Delete the damaged ${c.name}` });
        break;
      case "clash":
        tag = h("span", { class: "tag tag-rev", text: "Another file has this name" });
        hint = `A different file named ${c.file} is on this computer. Delete it in On this computer, then download the course one.`;
        break;
      case "failed":
        tag = h("span", { class: "tag tag-take", text: "Download failed" });
        hint = st.dl.error ? `${st.dl.error.title} ${st.dl.error.hint || ""}`.trim() : null;
        act = btn("btn-secondary", "Try again", () => resumeDownload(st.dl.id), { "aria-label": `Try downloading ${c.name} again`, disabled: hub.online !== true });
        break;
      case "paused":
        tag = h("span", { class: "tag tag-rev", text: "Paused" });
        act = btn("btn-secondary", "Resume", () => resumeDownload(st.dl.id), { "aria-label": `Resume downloading ${c.name}`, disabled: hub.online !== true });
        break;
      case "downloading":
        act = h("span", { class: "dl-status course-pct", "data-key": key, tabindex: "-1" });
        break;
      default: // missing, offline, checking
        act = h(
          "button",
          { class: "btn btn-primary btn-small", type: "button", "data-key": key, disabled: st.kind !== "missing", "aria-label": `Download ${c.name}, ${fmt.size(c.size)}`, onclick: () => startDownload("download_course_model", { id: c.id }) },
          svg(ICON.download),
          "Download"
        );
    }
    return h(
      "li",
      { class: "row course-row", "data-course": c.id, "data-state": st.kind },
      h(
        "div",
        { class: "row-main" },
        h("span", { class: "row-name", text: c.name }),
        h("span", { class: "row-meta" }, fmt.size(c.size), fitEl(c.fit), tag),
        h("span", { class: "row-hint course-note", text: c.note }),
        hint && h("span", { class: "row-hint", text: hint })
      ),
      h("div", { class: "row-acts" }, act)
    );
  }

  function renderCourse() {
    const el = hubEls.course;
    if (!el) return;
    el.hidden = !S.course_models.length || !!(hub.details || hub.loading);
    if (el.hidden) return;
    if (!el.firstChild) {
      el.ui = { offline: h("p", { class: "muted", text: "Connect to the internet to download them, or import them from the USB stick in On this computer." }), list: h("ul", { class: "model-list" }) };
      put(el, [h("h3", { class: "sub-h", id: "courseTitle", text: "For the course" }), h("p", { class: "muted", text: "The exact files the course uses, checked by Trurl. Download these rather than searching." }), el.ui.offline, el.ui.list]);
    }
    const states = S.course_models.map((c) => [c, courseState(c)]);
    el.ui.offline.hidden = !(hub.online === false && states.some(([, st]) => st.kind === "offline"));
    const list = el.ui.list;
    // Same models in the same order: patch the rows; anything else: start over.
    if ([...list.children].map((li) => li.dataset.course).join() !== S.course_models.map((c) => c.id).join()) list.replaceChildren();
    states.forEach(([c, st], i) => {
      const old = list.children[i];
      // Try again and Resume wait for a connection, like Download: redraw when it changes.
      const sig = st.kind + (st.dl ? ":" + st.dl.id + ":" + hub.online : "");
      if (!old || old.dataset.sig !== sig) {
        keepFocus(list, () => {
          const li = courseRow(c, st);
          li.dataset.sig = sig;
          if (old) old.replaceWith(li);
          else list.append(li);
        });
      }
      if (st.kind === "downloading" || st.kind === "paused") {
        const [word, tone] = DL_WORDS[st.dl.status] || [st.dl.status, ""];
        const pct = list.children[i].querySelector(".course-pct");
        if (pct) pct.textContent = `${word} ${fmt.pct(st.dl.done, st.dl.size)}%`;
        if (pct) pct.className = "dl-status course-pct " + tone;
      }
    });
  }

  async function checkHub() {
    hub.online = null;
    renderHub();
    hub.online = await api.call("hub_online").catch(() => false);
    renderHub();
  }

  function renderHub() {
    const { status, search: searchEl, results, details } = hubEls;
    const showSearch = hub.online === true && !hub.details && !hub.loading;
    searchEl.hidden = !showSearch;
    results.hidden = !showSearch;
    details.hidden = !(hub.online && (hub.details || hub.loading));
    if (hub.online === null) {
      status.replaceChildren(h("p", { class: "state-msg" }, spinner(), "Checking your internet connection…"));
    } else if (hub.online === false) {
      status.replaceChildren(
        h(
          "div",
          { class: "offline" },
          h("strong", { text: "Connect to the internet to browse Hugging Face" }),
          h("p", { text: "Everything else still works without internet, including chatting with the models on this computer." }),
          h("button", { class: "btn btn-secondary", type: "button", onclick: checkHub }, "Check again")
        )
      );
    } else status.replaceChildren();
    status.hidden = !status.firstChild;

    if (showSearch) {
      if (hub.searching) results.replaceChildren(h("p", { class: "state-msg" }, spinner(), "Searching Hugging Face…"));
      else if (hub.error) results.replaceChildren(h("div", { class: "inline-note take" }, h("p", { class: "t", text: hub.error.title }), h("p", { class: "h", text: hub.error.hint || "" })));
      else if (hub.results && !hub.results.length) results.replaceChildren(h("p", { class: "empty-note", text: `No models found for “${hub.query}”. Try a shorter name.` }));
      else if (hub.results) {
        results.replaceChildren(
          h("p", { class: "muted", text: "Choose a model to see its files. Models marked “Does not chat” cannot answer questions." }),
          h(
            "ul",
            { class: "hits" },
            [...hub.results.filter((r) => r.chat !== false), ...hub.results.filter((r) => r.chat === false)].map((r) =>
              h(
                "li",
                null,
                h(
                  "button",
                  { class: "hit hub-hit", type: "button", "data-key": "hit:" + r.id, "data-not-chat": r.chat === false ? "" : null, onclick: () => openDetails(r.id) },
                  h("span", { class: "row-name", text: r.id }),
                  h(
                    "span",
                    { class: "row-meta" },
                    h("span", { text: `${fmt.num(r.downloads)} downloads` }),
                    h("span", { text: `${fmt.num(r.likes)} likes` }),
                    r.updated && h("span", { text: `Updated ${fmt.date(r.updated)}` }),
                    r.gated && h("span", { class: "tag tag-rev", text: "Needs an account" }),
                    r.chat === false && h("span", { class: "not-chat", text: "Does not chat" })
                  )
                )
              )
            )
          )
        );
      } else results.replaceChildren();
    }
    renderCourse();
    if (hub.loading) details.replaceChildren(backButton(), h("p", { class: "state-msg" }, spinner(), `Loading ${hub.loading}…`));
    else if (hub.details) renderDetails();
  }

  async function search(q) {
    hub.query = q.trim();
    hub.searching = true;
    hub.error = null;
    renderHub();
    try {
      hub.results = await api.call("hub_search", { query: hub.query });
    } catch (e) {
      if (e.code === "offline") hub.online = false;
      hub.error = e;
      hub.results = null;
    }
    hub.searching = false;
    renderHub();
  }

  function backButton() {
    return h(
      "button",
      {
        class: "btn btn-quiet back",
        type: "button",
        onclick: () => {
          hub.details = hub.loading = null;
          renderHub();
          const last = hubEls.results.querySelector(`[data-key="${CSS.escape("hit:" + hub.lastHit)}"]`);
          (last || hubEls.qInput).focus();
        },
      },
      svg(ICON.back),
      "Back to results"
    );
  }

  async function openDetails(repo) {
    hub.lastHit = repo;
    hub.loading = repo;
    hub.details = null;
    renderHub();
    hubEls.details.querySelector("button").focus();
    try {
      const d = await api.call("hub_details", { repo });
      if (hub.loading !== repo) return;
      hub.details = d;
    } catch (e) {
      notice(e);
    }
    hub.loading = null;
    renderHub();
    const back = hubEls.details.querySelector(".back");
    if (back) back.focus();
  }

  function renderDetails() {
    const d = hub.details;
    const [author, name] = d.id.includes("/") ? d.id.split("/") : [d.author, d.id];
    const fact = (k, v) => h("div", null, h("dt", { text: k }), h("dd", { text: v || "Not listed" }));
    hubEls.files = h("tbody");
    put(hubEls.details, [
      backButton(),
      h("div", { class: "repo-head" }, h("h3", { text: name }), h("p", null, "by ", h("span", { class: "mono", text: d.author || author }), d.gated && " ", d.gated && h("span", { class: "tag tag-rev", text: "Needs an account" }))),
      h(
        "dl",
        { class: "facts" },
        fact("License", d.license),
        fact("Parameters", fmt.params(d.parameters)),
        fact("Architecture", d.architecture),
        fact("Context length", d.context_length ? fmt.tokens(d.context_length) : null),
        fact("Last updated", fmt.date(d.updated)),
        fact("Base model", d.base_model),
        fact("Downloads", fmt.num(d.downloads)),
        fact("Likes", fmt.num(d.likes))
      ),
      d.chat === false &&
        h(
          "div",
          { class: "inline-note take", role: "note" },
          h("p", { class: "t", text: "This model does not chat." }),
          h("p", { class: "h", text: "It turns text into numbers, or works with sound. It cannot answer questions in Trurl. Look for a model whose name ends in Instruct or Chat." })
        ),
      ...[usefulTags(d.tags)].filter((t) => t.length).map((t) => h("div", { class: "tags", "aria-label": "Tags" }, t.map((x) => h("span", { class: "tag", text: x })))),
      d.gated && !(S.settings && S.settings.has_hf_token) && h("p", { class: "gate-note", text: "This model needs a free Hugging Face account. Ask your teacher to add an access token (Ctrl+Shift+S)." }),
      h("h4", { class: "sub-h", text: "Files" }),
      h("p", { class: "muted", text: "Each file is the same model at a different size. Smaller files are faster and use less memory." }),
      h("p", { class: "muted quant-legend", text: quantLegend(d.files, (S.hardware || {}).variant) }),
      h("table", { class: "files" }, h("thead", null, h("tr", null, h("th", { text: "Version" }), h("th", { text: "Size" }), h("th", { text: "Fits this computer?" }), h("th", null, h("span", { class: "sr-only", text: "Action" })))), hubEls.files),
      h("h4", { class: "sub-h", text: "Model card" }),
    ]);
    const card = h("div", { class: "card-md md" });
    if (d.readme) card.innerHTML = md(d.readme);
    else card.append(h("p", { class: "muted", text: "This model has no model card." }));
    hubEls.details.append(card);
    renderFiles();
  }

  // Same rule as hub::recommend: Q4_0 is the fast format on the ssse3 build only.
  function quantLegend(files, variant) {
    const rec = (files.find((f) => f.recommended) || {}).quant;
    const base = "The version name says how much the model was squeezed: Q4 keeps about 4 bits per number (small, good), Q8 keeps 8 (twice the size, a little more exact). Names starting with IQ or UD are other ways of squeezing.";
    if (rec === "Q4_0" && variant === "ssse3") return base + " On this computer's processor, Q4_0 files answer fastest: the recommended one is listed first.";
    if (rec) return base + ` The recommended one, ${rec}, is listed first.`;
    return base;
  }

  // Tags a learner can use (the language, the license); Hugging Face's
  // technical ones (endpoints_compatible, region:us, base_model:...) are noise.
  function usefulTags(tags) {
    const noise = /^(endpoints_compatible|conversational|gguf|region:|base_model:|license:|arxiv:|autotrain|text-generation-inference|imatrix|transformers|safetensors|deploy:|custom_code|template:|dataset:|doi:|model-index|has_space|eval-results|.*-generation$)/;
    return (tags || []).filter((t) => !noise.test(t)).slice(0, 12);
  }

  function renderFiles() {
    const d = hub.details;
    if (!d || !hubEls.files) return;
    // Downloads tick several times a second. Each row's action cell is replaced
    // only when that row's state changes, so a button is never swapped under the
    // pointer of a learner about to click it.
    const rowState = (f) => {
      const dest = basename(f.path);
      const dl = S.downloads.find((x) => x.dest_file === dest);
      return f.split ? "split" : S.models.some((m) => m.file === dest) ? "here" : dl ? dl.status : "";
    };
    const action = (f) => {
      const dest = basename(f.path);
      const st = rowState(f);
      if (st === "split") return h("span", { class: "muted", text: "Split file, not supported" });
      if (st === "here") return h("span", { class: "tag tag-held", text: "On this computer" });
      if (st) return h("span", { class: "tag tag-a", text: DL_WORDS[st] ? DL_WORDS[st][0] : st });
      return h(
        "button",
        { class: "btn btn-secondary btn-small", type: "button", "data-key": "dl:" + f.path, "aria-label": `Download ${f.quant || dest}, ${fmt.size(f.size)}`, onclick: () => startDownload("download_start", { repo: d.id, file: f.path, revision: d.revision, size: f.size, sha256: f.sha256 || null }) },
        svg(ICON.download),
        "Download"
      );
    };
    const tb = hubEls.files;
    if (tb.dataset.repo !== d.id || tb.children.length !== d.files.length) {
      tb.dataset.repo = d.id;
      // The core lists the file to pick on this computer first, marked recommended.
      keepFocus(tb, () =>
        tb.replaceChildren(
          ...d.files.map((f) =>
            h(
              "tr",
              { "data-state": rowState(f) },
              h("td", null, h("span", { class: "q", text: f.quant || "Unknown" }), f.recommended && h("span", { class: "tag tag-held rec", text: "Recommended" }), h("span", { class: "p", text: f.path })),
              h("td", { class: "num", text: fmt.size(f.size) }),
              h("td", null, fitEl(f.fit)),
              h("td", null, action(f))
            )
          )
        )
      );
      return;
    }
    d.files.forEach((f, i) => {
      const tr = tb.children[i];
      const st = rowState(f);
      if (tr.dataset.state === st) return;
      tr.dataset.state = st;
      keepFocus(tr, () => tr.lastElementChild.replaceChildren(action(f)));
    });
  }

  // ---------- Chats: the history ------------------------------------------
  // Saved by the core after every answer, on this computer only. Opening one
  // brings back its questions, answers and settings, and the chat carries on.

  function when(secs) {
    const d = new Date(secs * 1000);
    const now = new Date();
    const time = d.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
    if (d.toDateString() === now.toDateString()) return `Today, ${time}`;
    const y = new Date(now);
    y.setDate(now.getDate() - 1);
    if (d.toDateString() === y.toDateString()) return `Yesterday, ${time}`;
    return d.toLocaleDateString("en-GB", { day: "numeric", month: "short", year: d.getFullYear() === now.getFullYear() ? undefined : "numeric" });
  }

  // Errors from this dialog are shown in it: the page's notices sit under it.
  function chatsNote(e) {
    put($("chatsNote"), e && h("div", { class: "inline-note take" }, h("p", { class: "t", text: e.title }), h("p", { class: "h", text: e.hint || "" })));
  }
  const chatTitle = (c) => c.title || "Untitled chat";

  async function openChat(c) {
    chatsNote(null);
    if (S.generating) {
      const go = await confirmBox({ title: "Stop the answer and open this chat?", text: "The answer being written stops here. It stays in its chat, so you can read it later.", ok: "Open the chat", keep: "Keep this answer" });
      if (!go) return;
    }
    const ok = await api.call("open_chat", { id: c.id }).then(() => true, (e) => (chatsNote(e), false));
    if (ok && $("chatsDlg").open) $("chatsDlg").close();
    if (ok) announce(`Opened the chat: ${chatTitle(c)}`);
  }

  async function deleteChat(c) {
    chatsNote(null);
    const stops = c.id === S.chat_id && S.generating ? " The answer being written stops." : "";
    const go = await confirmBox({ title: "Delete this chat?", text: `“${chatTitle(c)}” (${c.turns} question${c.turns === 1 ? "" : "s"}) is deleted from this computer. This cannot be undone.${stops}`, ok: "Delete", danger: true });
    if (go) api.call("delete_chat", { id: c.id }).catch(chatsNote);
  }

  function renderChats() {
    const list = $("chatList");
    if (!list) return;
    $("deleteAllChats").hidden = !S.chats.length;
    keepFocus(list, () => {
      if (!S.chats.length) {
        list.replaceChildren(h("li", { class: "empty-note", text: "No saved chats yet. A chat is saved here after its first answer." }));
        return;
      }
      list.replaceChildren(
        ...S.chats.map((c) => {
          const here = c.id === S.chat_id;
          return h(
            "li",
            { class: "row chat-row" + (here ? " current" : ""), "data-chat": c.id },
            h(
              "div",
              { class: "row-main" },
              h("span", { class: "row-name", text: chatTitle(c) }),
              h(
                "span",
                { class: "row-meta" },
                h("span", { text: when(c.updated) }),
                h("span", { text: `${c.turns} question${c.turns === 1 ? "" : "s"}` }),
                c.model && h("span", { text: c.model }),
                here && h("span", { class: "tag tag-held", text: "On screen" })
              )
            ),
            h(
              "div",
              { class: "row-acts" },
              !here && h("button", { class: "btn btn-secondary btn-small", type: "button", "data-key": "open:" + c.id, "aria-label": `Open the chat ${chatTitle(c)}`, onclick: () => openChat(c) }, "Open"),
              h("button", { class: "btn btn-danger-quiet btn-small", type: "button", "data-key": "del:" + c.id, "aria-label": `Delete the chat ${chatTitle(c)}`, onclick: () => deleteChat(c) }, "Delete")
            )
          );
        })
      );
    });
  }

  function openChats() {
    chatsNote(null);
    renderChats();
    openDialog($("chatsDlg"));
    const first = $("chatList").querySelector("button") || $("chatsDlg").querySelector("[data-close]");
    first.focus();
  }
  $("openChats").addEventListener("click", openChats);
  $("deleteAllChats").addEventListener("click", async () => {
    const go = await confirmBox({ title: "Delete all chats?", text: `All ${S.chats.length} saved chat${S.chats.length === 1 ? "" : "s"} are deleted from this computer, and the chat on screen is cleared. This cannot be undone.`, ok: "Delete all chats", danger: true });
    if (go) api.call("delete_all_chats").catch(chatsNote);
  });

  // ---------- Models dialog ----------------------------------------------

  const tabs = { local: $("tabLocal"), hub: $("tabHub") };
  function setTab(name) {
    for (const [k, tab] of Object.entries(tabs)) {
      const on = k === name;
      tab.setAttribute("aria-selected", String(on));
      tab.tabIndex = on ? 0 : -1;
      $(tab.getAttribute("aria-controls")).hidden = !on;
    }
    if (name === "hub" && hub.online !== true) checkHub();
  }
  Object.entries(tabs).forEach(([k, tab]) => {
    tab.addEventListener("click", () => setTab(k));
    tab.addEventListener("keydown", (e) => {
      if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
      const next = k === "local" ? "hub" : "local";
      setTab(next);
      tabs[next].focus();
    });
  });

  function openModels(tab) {
    renderLocal();
    renderDownloads();
    setTab(tab);
    openDialog($("modelsDlg"));
    tabs[tab].focus();
  }
  $("openModels").addEventListener("click", () => openModels("local"));
  $("dlChip").addEventListener("click", () => openModels("hub"));
  // ---------- Model dropdown (sidebar) ------------------------------------
  // One click on the card lists every model; one more switches. The full Models
  // screen (import, delete, Hugging Face) is one item away at the bottom.

  const menu = $("modelMenu");
  const optionsEl = $("modelOptions");

  function renderMenu() {
    const cur = currentModel();
    optionsEl.replaceChildren(
      ...S.models.map((m) => {
        const selected = !!cur && cur.file === m.file;
        const li = h(
          "li",
          {
            class: "model-option" + (selected ? " current" : ""),
            role: "option",
            id: "opt-" + m.file,
            "aria-selected": String(selected),
            "aria-disabled": m.broken ? "true" : null,
            "data-file": m.file,
          },
          h("span", { class: "mo-top" }, h("span", { class: "mo-name", text: m.name }), selected && h("span", { class: "mo-check", text: "In use" })),
          h("span", { class: "mo-meta" }, h("span", { text: [m.quant, fmt.size(m.size)].filter(Boolean).join(" · ") }), m.broken ? h("span", { class: "fit fit-red", text: "Damaged file" }) : fitEl(m.fit))
        );
        li.addEventListener("click", () => pick(m));
        return li;
      })
    );
  }

  function openMenu() {
    if (!S.models.length) return openModels("local");
    renderMenu();
    menu.hidden = false;
    $("modelCard").setAttribute("aria-expanded", "true");
    const cur = currentModel();
    setActive(cur ? S.models.findIndex((m) => m.file === cur.file) : 0);
    optionsEl.focus();
  }
  function closeMenu(focusCard = true) {
    if (menu.hidden) return;
    menu.hidden = true;
    $("modelCard").setAttribute("aria-expanded", "false");
    if (focusCard) $("modelCard").focus();
  }
  let active = 0;
  function setActive(i) {
    const items = [...optionsEl.children];
    if (!items.length) return;
    active = Math.max(0, Math.min(items.length - 1, i));
    items.forEach((el, k) => el.classList.toggle("active", k === active));
    optionsEl.setAttribute("aria-activedescendant", items[active].id);
    items[active].scrollIntoView({ block: "nearest" });
  }
  function pick(m) {
    if (m.broken) return;
    closeMenu();
    const cur = currentModel();
    if (cur && cur.file === m.file && S.engine.state === "ready") return;
    useModel(m);
  }
  $("modelCard").addEventListener("click", () => (menu.hidden ? openMenu() : closeMenu()));
  optionsEl.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") (setActive(active + 1), e.preventDefault());
    else if (e.key === "ArrowUp") (setActive(active - 1), e.preventDefault());
    else if (e.key === "Home") (setActive(0), e.preventDefault());
    else if (e.key === "End") (setActive(S.models.length - 1), e.preventDefault());
    else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      if (S.models[active]) pick(S.models[active]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closeMenu();
    } else if (e.key === "Tab") closeMenu(false);
  });
  $("menuManage").addEventListener("click", () => (closeMenu(false), openModels("local")));
  $("menuDownload").addEventListener("click", () => (closeMenu(false), openModels("hub")));
  document.addEventListener("mousedown", (e) => {
    if (!menu.hidden && !e.target.closest(".model-pick")) closeMenu(false);
  });

  // ---------- About -------------------------------------------------------

  const VARIANT = { avx2: "AVX2", avx: "AVX", ssse3: "SSSE3", compat: "Compatible" };
  let licenses = null;

  async function openAbout() {
    const body = $("aboutBody");
    const hw = S.hardware || {};
    const result = h("p", { class: "check-result", "aria-live": "polite" });
    const checkBtn = h("button", { class: "btn btn-secondary", type: "button" }, "Check for updates");
    checkBtn.addEventListener("click", async () => {
      checkBtn.disabled = true;
      result.replaceChildren(spinner(), " Checking…");
      try {
        const info = await api.call("update_check");
        if (info) {
          result.textContent = `Version ${info.version} is available. See the card above the chat.`;
          offerUpdate(info);
        } else result.textContent = "You have the latest version.";
      } catch (e) {
        result.textContent = e.code === "offline" ? "Connect to the internet to check for updates." : `${e.title} ${e.hint || ""}`.trim();
      }
      checkBtn.disabled = false;
    });
    const licList = h("div", { class: "licenses" }, h("p", { class: "state-msg" }, spinner(), "Loading licenses…"));
    const row = (k, ...v) => [h("dt", { text: k }), h("dd", null, ...v)];

    body.replaceChildren(
      h(
        "div",
        { class: "about-top" },
        h("img", { class: "logo", src: "img/trurl-logo.svg", alt: "Trurl" }),
        h("div", null, h("p", { class: "ver" }, "Version ", h("span", { class: "num", text: S.version })), h("p", { class: "by" }, "By Julien Barbier, for Cyberiad Labs", h("img", { src: "img/cyberiad-mark.svg", alt: "" })))
      ),
      h("p", { text: "Trurl lets you run language models on your own computer, without internet, and see how they work." }),
      h(
        "dl",
        { class: "about-list" },
        row("Model engine", h("span", { class: "mono", text: "llama.cpp " + S.llama_build })),
        row(
          "This computer",
          h("span", { class: "mono", text: `${hw.total_ram ? fmt.ram(hw.total_ram) : "?"} memory · ${hw.physical_cores || "?"} cores · ${VARIANT[hw.variant] || "Compatible"}` }),
          hw.cpu_name && h("span", { class: "muted", text: hw.cpu_name })
        ),
        row("Models folder", h("span", { class: "mono", text: S.models_dir }), h("button", { class: "btn btn-secondary", type: "button", "data-open-folder": "" }, "Open folder")),
        row("Updates", checkBtn, result)
      ),
      h("h3", { class: "sub-h", text: "Licenses" }),
      licList
    );
    openDialog($("aboutDlg"));
    $("aboutDlg").querySelector("[data-close]").focus();
    try {
      licenses = licenses || (await api.call("licenses"));
      licList.replaceChildren(...licenses.map((l) => h("details", null, h("summary", { text: l.name }), h("pre", { tabindex: "0", text: l.text }))));
    } catch (e) {
      licList.replaceChildren(h("p", { class: "muted", text: `${e.title} ${e.hint || ""}`.trim() }));
    }
  }
  $("openAbout").addEventListener("click", openAbout);

  // ---------- Hidden settings: Hugging Face token (Ctrl+Shift+S) ----------

  function renderToken() {
    const has = !!(S.settings && S.settings.has_hf_token);
    const st = $("tokenState");
    st.textContent = has ? "A token is saved on this computer." : "No token saved.";
    st.className = "token-state" + (has ? " held" : "");
    $("tokenRemove").disabled = !has;
  }
  function openToken() {
    $("tokenInput").value = "";
    renderToken();
    openDialog($("tokenDlg"));
    $("tokenInput").focus();
  }
  $("tokenForm").addEventListener("submit", async (e) => {
    e.preventDefault();
    const token = $("tokenInput").value.trim();
    if (!token) return $("tokenInput").focus();
    try {
      await call("set_hf_token", { token });
      $("tokenInput").value = "";
      announce("Token saved.");
    } catch (_) {}
  });
  $("tokenRemove").addEventListener("click", async () => {
    try {
      await call("set_hf_token", { token: "" });
      announce("Token removed.");
      $("tokenInput").focus();
    } catch (_) {}
  });

  // ---------- Narrow windows: sidebar behind a button ---------------------

  $("sideToggle").addEventListener("click", () => {
    const open = $("app").classList.toggle("side-open");
    $("sideToggle").setAttribute("aria-expanded", String(open));
  });

  // ---------- Keyboard ----------------------------------------------------

  document.addEventListener("keydown", (e) => {
    const anyDialog = document.querySelector("dialog[open]");
    const ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && e.shiftKey && e.key.toLowerCase() === "s") {
      e.preventDefault();
      if (!anyDialog) openToken();
    } else if (ctrl && !e.shiftKey && e.key.toLowerCase() === "n") {
      e.preventDefault();
      if (!anyDialog) newChat();
    } else if (e.key === "Escape" && !anyDialog) {
      if (S.generating) run("stop");
      else if ($("app").classList.contains("side-open")) $("sideToggle").click();
    }
  });

  // ---------- Events from the core ----------------------------------------

  function applySnapshot(s) {
    Object.assign(S, {
      version: s.version,
      llama_build: s.llama_build,
      models_dir: s.models_dir,
      hardware: s.hardware,
      settings: s.settings,
      models: s.models || [],
      engine: s.engine || { state: "idle" },
      generating: !!s.generating,
      downloads: s.downloads || [],
      course_models: s.course_models || [],
      chats: s.chats || [],
      chat_id: s.chat_id || null,
    });
    applySettings();
    resetTranscript(s.turns || []);
    renderDownloads();
    renderLocal();
    renderStatus();
    renderChats();
  }

  const handlers = {
    snapshot: (ev) => applySnapshot(ev.snapshot),
    engine(ev) {
      S.engine = ev.engine;
      if (queued && S.engine.state !== "loading") {
        // Ready: send what the learner typed while waiting. Failed: keep it in the box.
        queued = false;
        if (S.engine.state === "ready") send();
      }
      renderStatus();
      renderLocal();
      renderEmpty();
    },
    models(ev) {
      S.models = ev.models;
      if (!menu.hidden) renderMenu();
      renderLocal();
      renderStatus();
      renderEmpty();
      if (hub.details) renderFiles();
    },
    settings(ev) {
      S.settings = ev.settings;
      applySettings();
      renderToken();
      renderLocal();
    },
    generating(ev) {
      S.generating = ev.generating;
      renderStatus();
      renderUpdate();
      if (!ev.generating) announce("Answer finished.");
    },
    turn_started(ev) {
      if (views.has(ev.turn.id)) return;
      if (!views.size) turnsEl.replaceChildren();
      turnsEl.append(makeView(ev.turn).el);
      scrollToEnd();
    },
    turn_delta(ev) {
      const v = views.get(ev.id);
      if (!v) return;
      const t = v.turn;
      const wasAnswering = !!t.answer;
      const wasThinking = !!t.reasoning;
      if (ev.reasoning_from_answer) {
        t.reasoning += t.answer;
        t.answer = "";
      }
      if (ev.reasoning) t.reasoning += ev.reasoning;
      if (ev.answer) t.answer += ev.answer;
      schedule(v);
      // "Starting" -> "Thinking" -> "Answering": the status follows each first.
      if ((!wasAnswering && t.answer) || (!wasThinking && t.reasoning)) {
        renderStatus();
        renderComposer();
      }
    },
    turn_thought(ev) {
      const v = views.get(ev.id);
      if (!v) return;
      v.turn.thought_ms = ev.ms;
      follow(() => paint(v));
    },
    turn_finished(ev) {
      const v = views.get(ev.turn.id);
      if (!v) return;
      v.turn = { ...ev.turn };
      dirty.delete(v);
      follow(() => paint(v));
    },
    chat_cleared: () => resetTranscript([]),
    import_progress(ev) {
      S.importing = { file: ev.file, done: ev.done, total: ev.total };
      renderImport();
    },
    import_finished(ev) {
      S.importing = null;
      renderImport();
      if (ev.error) notice(ev.error);
      else announce(`${ev.file} is ready to use.`);
    },
    verify_progress(ev) {
      S.verifying = ev.done < ev.total ? { file: ev.file, done: ev.done, total: ev.total } : null;
      renderLocal();
    },
    downloads(ev) {
      S.downloads = ev.downloads;
      renderDownloads();
    },
    chats(ev) {
      S.chats = ev.chats;
      S.chat_id = ev.chat_id || null;
      renderChats();
    },
    notice: (ev) => notice(ev.error),
    update_available: (ev) => offerUpdate(ev.update),
    update_progress(ev) {
      if (!S.update || S.update.phase === "failed") return;
      Object.assign(S.update, { phase: "installing", done: ev.downloaded, total: ev.total });
      renderUpdate();
    },
    update_failed(ev) {
      if (!S.update) return;
      Object.assign(S.update, { phase: "failed", error: ev.error, hidden: false });
      renderUpdate();
    },
  };

  buildHub();
  api.on((ev) => {
    const fn = handlers[ev && ev.type];
    if (fn) fn(ev);
  });
  call("snapshot").then(applySnapshot, () => null);
  autosize();
  renderComposer();
})();
