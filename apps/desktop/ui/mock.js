// A scripted fake of trurl-core for design work, screenshots and UI tests.
// Open index.html?mock to use it. Add &scenario=<name> for a starting state:
//   default   three bundled models, Gemma 1B loading then ready
//   empty     no models yet (first run without the USB models folder)
//   offline   Hugging Face unreachable
//   failed    the model did not load (too big)
//   update    an update is offered a few seconds after start
//   autofail  the automatic update check got no answer (About says why)
//   missedevent  the automatic check found an update but the page missed the event
//   chat      a conversation already on screen, with reasoning
//   oldcpu    a 2008 laptop (SSSE3 engine): Hugging Face recommends Q4_0 files
//   coursestates  each course row state: damaged, failed download, a look-alike file
//   chats     three saved chats in the history
// Timings are short but real enough to see streaming and loading states.

(function () {
  "use strict";
  const GB = 1024 * 1024 * 1024;

  function create(emit) {
    const params = new URLSearchParams(location.search);
    const scenario = params.get("scenario") || "default";
    const later = (ms, fn) => setTimeout(fn, ms);
    const err = (code, title, hint) => ({ code, title, hint });
    const UPDATE = { version: "0.2.0", notes: "Faster model loading.\nClearer error when a model is too big.", date: "2026-11-02" };
    let autoStatus = null; // what the last automatic update check did (`update_status`)

    const models = [
      { file: "gemma-3-1b-it-Q4_K_M.gguf", name: "Gemma 3 1B Instruct", quant: "Q4_K_M", size: 806058240, fit: "green", bundled: true, course: true, verified: true, broken: false, has_chat_template: true, reasoning_hint: false, train_context: 32768 },
      { file: "Qwen_Qwen3-0.6B-Q4_K_M.gguf", name: "Qwen3 0.6B", quant: "Q4_K_M", size: 484220320, fit: "green", bundled: true, course: true, verified: true, broken: false, has_chat_template: true, reasoning_hint: true, train_context: 40960 },
      { file: "gemma-3-4b-it-Q4_K_M.gguf", name: "Gemma 3 4B Instruct", quant: "Q4_K_M", size: 2489757856, fit: "orange", bundled: true, course: false, verified: true, broken: false, has_chat_template: true, reasoning_hint: false, train_context: 131072 },
    ];
    // As pins.json: the three models the course uses, in course order.
    const COURSE = [
      { id: "gemma-3-1b-it", name: "Gemma 3 1B Instruct", note: "The model you use for the whole course.", file: "gemma-3-1b-it-Q4_K_M.gguf", size: 806058240, fit: "green", repo: "ggml-org/gemma-3-1b-it-GGUF" },
      { id: "gemma-3-1b-pt", name: "Gemma 3 1B PT", note: "Optional. The same model before it was taught to answer: it continues your text and does not stop by itself, so you stop it.", file: "gemma-3-1b-pt.Q4_K_M.gguf", size: 806056864, fit: "green", repo: "mradermacher/gemma-3-1b-pt-GGUF" },
      { id: "qwen3-0.6b", name: "Qwen3 0.6B", note: "A model trained to think before it answers. You watch its thinking as it writes.", file: "Qwen_Qwen3-0.6B-Q4_K_M.gguf", size: 484220320, fit: "green", repo: "bartowski/Qwen_Qwen3-0.6B-GGUF" },
    ];
    const state = {
      version: "0.1.0",
      llama_build: "v0.6.0 (b11429)",
      models_dir: "C:\\Users\\Ama\\AppData\\Local\\CyberiadLabs\\Trurl\\models",
      hardware: { total_ram: 4 * GB, physical_cores: 2, threads: 2, cpu_name: scenario === "oldcpu" ? "Intel(R) Core(TM)2 Duo CPU T6600 @ 2.20GHz" : "Intel(R) Core(TM) i5-3320M CPU @ 2.60GHz", variant: scenario === "oldcpu" ? "ssse3" : "avx" },
      settings: { system_prompt: "", temperature: 0.7, context_size: 2048, model: models[0].file, has_hf_token: false, auto_update_check: true, default_temperature: 0.7, default_context: 2048, context_choices: [256, 512, 1024, 2048, 4096, 8192] },
      models: scenario === "empty" ? [] : models.slice(),
      engine: { state: "idle" },
      turns: [],
      generating: false,
      downloads: [],
      course_models: COURSE.map(({ repo, ...c }) => ({ ...c, download_id: (repo + c.file).replace(/\W/g, "").slice(-16) })),
      chats: [],
      chat_id: null,
    };
    let nextTurn = 1;
    // Saved chats, as the core's history: id -> { id, created, updated, turns }.
    const saved = new Map();
    const DAY = 86400;
    const nowS = () => Math.floor(Date.now() / 1000);
    const summary = (c) => ({ id: c.id, title: (c.turns[0].user.split("\n").find((l) => l.trim()) || "").trim().slice(0, 60), updated: c.updated, turns: c.turns.length, model: c.turns[c.turns.length - 1].model });
    const pushChats = () => {
      state.chats = [...saved.values()].sort((a, b) => b.updated - a.updated).map(summary);
      emit({ type: "chats", chats: state.chats, chat_id: state.chat_id });
    };
    let chatSeq = 1;
    function saveChat() {
      if (!state.turns.length) return;
      if (!state.chat_id) state.chat_id = `${Date.now()}-${String(chatSeq++).padStart(4, "0")}`;
      const old = saved.get(state.chat_id);
      saved.set(state.chat_id, { id: state.chat_id, created: old ? old.created : nowS(), updated: nowS(), turns: state.turns.map((t) => ({ ...t, status: t.status === "streaming" ? "stopped" : t.status })) });
      pushChats();
    }
    if (scenario === "chats") {
      const t = (user, answer, model, extra = {}) => ({ id: 0, user, reasoning: "", answer, model, status: "done", thought_ms: null, system_prompt: "", temperature: 0.7, context: 2048, ...extra });
      [
        { id: "1700000000000-0001", created: nowS() - 3 * DAY, updated: nowS() - 3 * DAY, turns: [t("Explain the moon landing to a 6 year old in a few sentences.", "Okay, imagine the Earth is like a giant bouncy ball…", "Gemma 3 1B Instruct")] },
        { id: "1700000000000-0002", created: nowS() - DAY, updated: nowS() - DAY + 600, turns: [t("What is 17 + 25?", "**42**", "Qwen3 0.6B"), t("And 42 + 58?", "**100**", "Qwen3 0.6B")] },
        { id: "1700000000000-0003", created: nowS() - 3600, updated: nowS() - 1800, turns: [t("Hello! Who are you?", "Arr, I be a friendly pirate…", "Gemma 3 1B Instruct", { system_prompt: "You are a friendly pirate. Always start your answer with the word Arr.", temperature: 0.2 })] },
      ].forEach((c) => saved.set(c.id, c));
      state.chats = [...saved.values()].sort((a, b) => b.updated - a.updated).map(summary);
    }
    let cancel = null;
    const snap = () => JSON.parse(JSON.stringify(state));
    const setEngine = (e) => {
      state.engine = e;
      emit({ type: "engine", engine: e });
    };
    const modelBy = (f) => state.models.find((m) => m.file === f);

    function load(file) {
      const m = modelBy(file);
      if (!m) return;
      state.settings.model = file;
      emit({ type: "settings", settings: snap().settings });
      setEngine({ state: "loading", file, name: m.name });
      later(scenario === "failed" ? 1200 : 1600 + m.size / 1e9 * 800, () => {
        if (state.settings.model !== file) return;
        if (scenario === "failed" || m.fit === "red") {
          setEngine({ state: "failed", file, name: m.name, error: err("model_too_big", `${m.name} is too big for this computer's memory.`, "Close other apps and try again, or pick a smaller model (green dot) and a smaller context size.") });
        } else {
          setEngine({ state: "ready", file, name: m.name, context: state.settings.context_size });
        }
      });
    }

    const REASONING = "The learner asks for 17 + 25. Let me add the tens first: 10 + 20 = 30. Then the units: 7 + 5 = 12. So 30 + 12 = 42. Let me check another way: 25 + 17, 25 + 10 = 35, 35 + 7 = 42. Both give 42.";
    const ANSWERS = {
      default: "Here is a short answer.\n\n**Accra** is the capital of Ghana. It sits on the Atlantic coast and has about 2.5 million people.\n\n- It became the capital in 1877.\n- The main language is English, with Twi and Ga widely spoken.",
      math: "**42**",
      code: "Here is how you would print it in Python:\n\n```python\nfor i in range(3):\n    print(i)\n```\n\nThis prints 0, 1 and 2.",
    };

    function stream(turn, reasoning, answer, onDone) {
      const words = (s) => s.match(/\S+\s*/g) || [];
      const rw = words(reasoning);
      const aw = words(answer);
      let i = 0;
      const t0 = Date.now();
      let stopped = false;
      cancel = () => (stopped = true);
      const step = () => {
        if (stopped) return onDone("stopped");
        if (i < rw.length) {
          turn.reasoning += rw[i];
          emit({ type: "turn_delta", id: turn.id, reasoning: rw[i] });
        } else if (i - rw.length < aw.length) {
          if (i === rw.length && rw.length) {
            turn.thought_ms = Date.now() - t0;
            emit({ type: "turn_thought", id: turn.id, ms: turn.thought_ms });
          }
          const w = aw[i - rw.length];
          turn.answer += w;
          emit({ type: "turn_delta", id: turn.id, answer: w });
        } else return onDone("done");
        i++;
        later(45, step);
      };
      later(300, step);
    }

    function send(text) {
      if (state.engine.state !== "ready") {
        return Promise.reject(state.engine.state === "idle" ? err("no_model", "No model is loaded.", "Pick a model in the list on the left. If the list is empty, import a model file or download one.") : err("busy", "Trurl is still busy with the last request.", "Wait for it to finish, or press Stop."));
      }
      if (state.generating) return Promise.reject(err("busy", "Trurl is still busy with the last request.", "Wait for it to finish, or press Stop."));
      const turn = { id: nextTurn++, user: text, reasoning: "", answer: "", model: state.engine.name, status: "streaming", thought_ms: null, system_prompt: state.settings.system_prompt, temperature: state.settings.temperature, context: state.settings.context_size };
      state.turns.push(turn);
      state.generating = true;
      emit({ type: "generating", generating: true });
      emit({ type: "turn_started", turn: { ...turn } });
      const finish = (status, error) => {
        turn.status = status;
        if (error) turn.error = error;
        state.generating = false;
        cancel = null;
        emit({ type: "turn_finished", turn: { ...turn } });
        emit({ type: "generating", generating: false });
        saveChat();
      };
      if (/LONG|ROOM/.test(text) || (state.settings.context_size <= 256 && text.length > 200)) {
        later(400, () => finish("error", err("context_full", "The conversation ran out of room.", "Start a new chat or increase the context size.")));
        return Promise.resolve(turn.id);
      }
      const thinks = state.engine.file.startsWith("Qwen") && !/\/no_think/.test(text);
      const ans = /\d+\s*\+\s*\d+/.test(text) ? ANSWERS.math : /code|python/i.test(text) ? ANSWERS.code : ANSWERS.default;
      stream(turn, thinks ? REASONING : "", ans, (status) => finish(status));
      return Promise.resolve(turn.id);
    }

    // ----- Hugging Face ----------------------------------------------------
    const HITS = [
      { id: "bartowski/Qwen_Qwen3-0.6B-GGUF", downloads: 82560, likes: 28, gated: false, updated: "2025-04-28T17:26:40.000Z" },
      { id: "unsloth/Qwen3-1.7B-GGUF", downloads: 61234, likes: 51, gated: false, updated: "2025-05-02T10:00:00.000Z" },
      { id: "ggml-org/gemma-3-1b-it-GGUF", downloads: 40211, likes: 77, gated: false, updated: "2025-03-12T10:30:05.000Z" },
      { id: "bartowski/Llama-3.2-1B-Instruct-GGUF", downloads: 35110, likes: 120, gated: false, updated: "2024-09-25T10:00:00.000Z" },
      { id: "TheBloke/phi-2-GGUF", downloads: 9120, likes: 210, gated: false, updated: "2023-12-18T10:00:00.000Z" },
    ];
    const README = "# Qwen3 0.6B (GGUF)\n\nQuantized by **bartowski** with llama.cpp.\n\nQwen3 is a family of models that can **think** before answering. Add `/no_think` to your message to turn thinking off.\n\n## Which file should I pick?\n\n| File | Size | Notes |\n|---|---|---|\n| Q4_K_M | 484 MB | Good balance, recommended |\n| Q8_0 | 805 MB | Highest quality |\n\n## License\n\nApache 2.0.";
    function details(repo) {
      // As hub::recommend: Q4_0 first on the ssse3 engine, Q4_K_M otherwise.
      const q40 = scenario === "oldcpu";
      const files = [
        { path: "Qwen_Qwen3-0.6B-Q4_K_M.gguf", size: 484220320, sha256: "b".repeat(64), quant: "Q4_K_M", fit: "green", split: false, recommended: !q40 },
        { path: "Qwen_Qwen3-0.6B-Q4_0.gguf", size: 428962880, sha256: "f".repeat(64), quant: "Q4_0", fit: "green", split: false, recommended: q40 },
        { path: "Qwen_Qwen3-0.6B-Q2_K.gguf", size: 347000000, sha256: "a".repeat(64), quant: "Q2_K", fit: "green", split: false },
        { path: "Qwen_Qwen3-0.6B-Q8_0.gguf", size: 804750592, sha256: "c".repeat(64), quant: "Q8_0", fit: "green", split: false },
        { path: "Qwen_Qwen3-0.6B-bf16.gguf", size: 1509347488, sha256: "d".repeat(64), quant: "BF16", fit: "green", split: false },
        { path: "big/Qwen-big-00001-of-00002.gguf", size: 3500000000, sha256: "e".repeat(64), quant: null, fit: "red", split: true },
      ];
      if (q40) files.unshift(files.splice(1, 1)[0]);
      return { id: repo, author: repo.split("/")[0], revision: "60b85c0e3d8fe0f6474f406922a26d12aca4550d", gated: false, license: "apache-2.0", downloads: 82560, likes: 28, updated: "2025-04-28T17:26:40.000Z", base_model: "Qwen/Qwen3-0.6B", architecture: "qwen3", context_length: 32768, parameters: 751632384, tags: ["gguf", "text-generation", "conversational"], readme: README, files };
    }

    const dl = new Map();
    const pushDownloads = () => {
      state.downloads = [...dl.values()].map((d) => ({ ...d }));
      emit({ type: "downloads", downloads: state.downloads });
    };
    // Every course row state at once: a damaged Gemma 3 1B, a failed PT download,
    // another file named like Qwen3 0.6B, and a checked non-course model.
    if (scenario === "coursestates") {
      state.models = [
        { ...models[0], broken: true, verified: false },
        { ...models[1], size: 123456789, bundled: false, course: false, verified: null, name: "Some other Qwen" },
        models[2],
      ];
      state.settings.model = null;
      const c = COURSE[1];
      const ptId = (c.repo + c.file).replace(/\W/g, "").slice(-16);
      dl.set(ptId, { id: ptId, repo: c.repo, file: c.file, revision: "pinned", dest_file: c.file, size: c.size, sha256: "pinned", done: c.size / 3, rate: 0, status: "failed", error: err("download_failed", "The download stopped.", "Check the connection, then try again.") });
      state.downloads = [...dl.values()].map((d) => ({ ...d }));
    }
    function runDownload(d) {
      d.status = "running";
      d.error = null;
      pushDownloads();
      const tick = () => {
        if (d.status !== "running") return;
        d.done = Math.min(d.size, d.done + d.size / 25);
        d.rate = 850000;
        if (d.done >= d.size / 2 && !d.dropped) {
          d.dropped = true;
          d.status = "retrying";
          d.rate = 0;
          pushDownloads();
          later(1500, () => {
            if (dl.has(d.id) && d.status === "retrying") runDownload(d);
          });
          return;
        }
        if (d.done >= d.size) {
          d.status = "verifying";
          d.rate = 0;
          pushDownloads();
          later(900, () => {
            dl.delete(d.id);
            pushDownloads();
            const name = d.dest_file;
            if (!modelBy(name)) {
              const c = COURSE.find((x) => x.file === name);
              state.models.push({ file: name, name: c ? c.name : name.replace(".gguf", "").replace(/[_-]/g, " "), quant: d.quant || null, size: d.size, fit: "green", bundled: !!c, course: !!c, verified: null, broken: false, has_chat_template: true, reasoning_hint: true, train_context: 32768 });
              emit({ type: "models", models: snap().models });
              if (state.engine.state === "idle") load(name);
            }
          });
          return;
        }
        pushDownloads();
        later(250, tick);
      };
      later(250, tick);
    }

    // ----- commands ----------------------------------------------------------
    const commands = {
      snapshot: () => snap(),
      send: ({ text }) => send(text),
      stop: () => {
        if (cancel) cancel();
      },
      new_chat: () => {
        if (cancel) cancel();
        saveChat();
        state.turns = [];
        state.chat_id = null;
        emit({ type: "chat_cleared" });
        pushChats();
      },
      open_chat: ({ id }) => {
        const c = saved.get(id);
        if (!c) return Promise.reject(err("chat_open", "This chat could not be opened.", "The file may be damaged. Your other chats are not affected."));
        if (cancel) cancel();
        saveChat();
        state.turns = c.turns.map((t) => ({ ...t, id: nextTurn++ }));
        state.chat_id = id;
        const last = c.turns[c.turns.length - 1];
        Object.assign(state.settings, { system_prompt: last.system_prompt || "", temperature: last.temperature ?? 0.7, context_size: last.context || 2048 });
        emit({ type: "snapshot", snapshot: snap() });
      },
      delete_chat: ({ id }) => {
        saved.delete(id);
        if (state.chat_id === id) {
          if (cancel) cancel();
          state.turns = [];
          state.chat_id = null;
          emit({ type: "chat_cleared" });
        }
        pushChats();
      },
      delete_all_chats: () => {
        if (cancel) cancel();
        saved.clear();
        state.turns = [];
        state.chat_id = null;
        emit({ type: "chat_cleared" });
        pushChats();
      },
      set_system_prompt: ({ text }) => {
        state.settings.system_prompt = text;
        emit({ type: "settings", settings: snap().settings });
      },
      set_temperature: ({ value }) => {
        state.settings.temperature = Math.round(Math.min(2, Math.max(0, value)) * 10) / 10;
        emit({ type: "settings", settings: snap().settings });
      },
      set_context: ({ value }) => {
        const changed = value !== state.settings.context_size;
        state.settings.context_size = value;
        emit({ type: "settings", settings: snap().settings });
        if (changed && state.settings.model) load(state.settings.model);
      },
      select_model: ({ file }) => load(file),
      reload: () => load(state.settings.model),
      import_model: ({ path }) => {
        const file = path.split(/[\\/]/).pop();
        let done = 0;
        const total = 900e6;
        const step = () => {
          done = Math.min(total, done + total / 12);
          emit({ type: "import_progress", file, done, total });
          if (done < total) return later(150, step);
          if (!modelBy(file)) state.models.push({ file, name: file.replace(".gguf", ""), quant: "Q4_K_M", size: total, fit: "green", bundled: false, verified: null, broken: false, has_chat_template: true, reasoning_hint: false, train_context: 8192 });
          emit({ type: "models", models: snap().models });
          emit({ type: "import_finished", file });
          if (state.engine.state === "idle") load(file);
        };
        later(100, step);
      },
      delete_model: ({ file }) => {
        state.models = state.models.filter((m) => m.file !== file);
        emit({ type: "models", models: snap().models });
        if (state.settings.model === file) {
          const next = state.models[0];
          if (next) load(next.file);
          else {
            state.settings.model = null;
            setEngine({ state: "idle" });
          }
        }
      },
      hub_online: () => new Promise((r) => later(500, () => r(scenario !== "offline"))),
      hub_search: ({ query }) =>
        new Promise((r, j) =>
          later(500, () => {
            if (scenario === "offline") return j(err("offline", "Connect to the internet to browse Hugging Face.", "Everything else in Trurl works without internet."));
            const q = (query || "").toLowerCase();
            r(HITS.filter((h) => !q || h.id.toLowerCase().includes(q.split(" ")[0])));
          })
        ),
      hub_details: ({ repo }) => new Promise((r) => later(600, () => r(details(repo)))),
      download_start: ({ repo, file, revision, size, sha256 }) => {
        const id = (repo + file).replace(/\W/g, "").slice(-16);
        const dest = file.split("/").pop();
        const d = { id, repo, file, revision, dest_file: dest, size, sha256, done: 0, rate: 0, status: "running", quant: null };
        dl.set(id, d);
        runDownload(d);
        return id;
      },
      download_course_model: ({ id }) => {
        const c = COURSE.find((x) => x.id === id);
        if (!c) throw new Error("no course model " + id);
        const here = modelBy(c.file);
        if (here && here.size === c.size) return Promise.reject(err("download_failed", "This model is already on this computer.", "Choose it in Models. To download it again, delete it there first."));
        const id2 = commands.download_start({ repo: c.repo, file: c.file, revision: "pinned", size: c.size, sha256: "pinned" });
        dl.get(id2).quant = "Q4_K_M";
        return id2;
      },
      download_pause: ({ id }) => {
        const d = dl.get(id);
        if (d) {
          d.status = "paused";
          d.rate = 0;
          pushDownloads();
        }
      },
      download_resume: ({ id }) => {
        const d = dl.get(id);
        if (d) runDownload(d);
      },
      download_cancel: ({ id }) => {
        dl.delete(id);
        pushDownloads();
      },
      set_hf_token: ({ token }) => {
        state.settings.has_hf_token = !!token.trim();
        emit({ type: "settings", settings: snap().settings });
      },
      set_auto_update: ({ on }) => {
        state.settings.auto_update_check = !!on;
        emit({ type: "settings", settings: snap().settings });
      },
      licenses: () => [
        { name: "Trurl (MIT)", text: "MIT License\n\nCopyright (c) 2026 Cyberiad Labs\n\nPermission is hereby granted, free of charge, …" },
        { name: "Gemma Terms of Use", text: "Gemma is provided under and subject to the Gemma Terms of Use found at ai.google.dev/gemma/terms\n\n…" },
        { name: "Qwen3 (Apache 2.0)", text: "Apache License, Version 2.0 …" },
        { name: "llama.cpp (MIT)", text: "MIT License\n\nCopyright (c) 2023-2026 The ggml authors …" },
      ],
      open_models_folder: () => {},
      // &checkfails: the manual check fails (GitHub unreachable), whatever is known.
      update_check: () =>
        new Promise((r, no) =>
          later(700, () => (params.has("checkfails") ? no(err("update_check_failed", "Trurl could not check for updates.", "Connect to the internet and try again. Trurl works the same without updates.")) : r(scenario === "update" || scenario === "missedevent" ? UPDATE : null)))
        ),
      // The last automatic check, as the desktop app reports it.
      update_status: () => autoStatus,
      update_install: () => {
        let d = 0;
        const total = 24e6;
        const step = () => {
          d = Math.min(total, d + total / 20);
          emit({ type: "update_progress", downloaded: d, total });
          if (d < total) later(150, step);
        };
        step();
      },
    };

    // Starting state.
    later(50, () => {
      emit({ type: "snapshot", snapshot: snap() });
      if (scenario === "chat") {
        state.engine = { state: "ready", file: models[1].file, name: models[1].name, context: 2048 };
        state.settings.model = models[1].file;
        state.turns = [
          { id: nextTurn++, user: "What is 17 + 25?", reasoning: REASONING, answer: "**42**", model: "Qwen3 0.6B", status: "done", thought_ms: 12400 },
          { id: nextTurn++, user: "Where is Accra?", reasoning: "", answer: ANSWERS.default, model: "Gemma 3 1B Instruct", status: "done", thought_ms: null },
        ];
        emit({ type: "snapshot", snapshot: snap() });
      } else if (state.models.length && state.settings.model) {
        load(state.settings.model);
      }
      // The automatic check, as the desktop app runs it: the result lands in
      // `update_status`, and (unless the page missed it) as `update_available`.
      if (scenario === "update" || scenario === "missedevent" || scenario === "autofail")
        later(scenario === "update" ? 2500 : 300, () => {
          const at = Math.floor(Date.now() / 1000);
          if (scenario === "autofail") {
            autoStatus = { at, outcome: "failed", retry_in_mins: 30, error: { code: "update_check_failed", title: "Trurl could not check for updates.", hint: "Connect to the internet and try again. Trurl works the same without updates.", detail: "error sending request for url (https://github.com/jbarbier/trurl/releases/latest/download/latest.json): operation timed out" } };
            return;
          }
          autoStatus = { at, outcome: "newer", update: UPDATE, retry_in_mins: 1440 };
          if (scenario === "update") emit({ type: "update_available", update: UPDATE });
        });
      else autoStatus = { at: Math.floor(Date.now() / 1000), outcome: "latest", retry_in_mins: 1440 };
    });

    return {
      kind: "mock",
      call: (cmd, args) => {
        const fn = commands[cmd];
        if (!fn) return Promise.reject(err("internal", "Unknown command " + cmd, ""));
        try {
          return Promise.resolve(fn(args || {}));
        } catch (e) {
          return Promise.reject(err("internal", String(e), ""));
        }
      },
      pickModelFile: async () => "E:\\Trurl-USB\\models\\phi-4-mini-instruct-Q4_K_M.gguf",
    };
  }

  window.TrurlMock = { create };
})();
