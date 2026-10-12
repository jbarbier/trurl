# Trurl architecture

How Trurl is built and why. For using Trurl, see the [README](../README.md).

## Goals, in priority order

1. **Same behavior in every class.** Every binary, library and model is pinned by
   version and SHA-256 in [`pins.json`](../pins.json). Nothing auto-updates except the
   app itself, and only when the teacher's learner clicks "Update now".
2. **Works offline on a 4 GB laptop.** No network on the chat path. Low-RAM defaults.
3. **Teaches, does not hide.** The learner sees what the model sees: no injected
   prompts, the model's own chat template, reasoning shown apart from the answer.
4. **Small and simple.** Only the controls the course needs.

## The pieces

```
                 ┌──────────────────────── trurl-core (Rust library) ───────────────────────┐
 apps/desktop ──▶│ Engine        spawns and supervises llama-server (one model at a time)  │
 (Tauri + HTML)  │ Chat          streams /v1/chat/completions, splits reasoning from answer │
                 │ Models        models folder, GGUF metadata, size and RAM-fit            │
 apps/tui ──────▶│ Hub           Hugging Face search + file list (only when online)        │
 (ratatui)       │ Downloads     resumable HTTP Range downloads, SHA-256 verify            │
                 │ Settings      per-model system prompt, temperature, context; HF token   │
 apps/bridge ───▶│ Hardware      RAM, physical cores, CPU features → llama build variant   │
 (dev only)      │ Update        latest.json check (desktop installs via Tauri updater)    │
                 └──────────────────────────────┬───────────────────────────────────────────┘
                                                │ 127.0.0.1:<random port>, random API key
                                         llama-server (pinned llama.cpp build)
```

Both front ends call the same `trurl_core::App` API and receive the same `Event`
stream (`contracts/events.md`). The desktop UI is plain HTML, CSS and JavaScript, with
no framework and no bundler. Its only dependencies are two vendored, pinned files for
Markdown (`marked`, `DOMPurify`). `apps/bridge` serves that same UI against the real
core over HTTP, so the UI can be driven and screenshotted in a browser on any OS.

## llama-server

- **Pinned build:** llama.cpp `v0.6.0` (build `b11429`), the official CPU release zips
  for Windows x64 and Ubuntu x64.
- **Two builds, chosen at startup.** The official CPU release ships one `ggml-cpu-*`
  library per CPU generation. `scripts/fetch_deps.py` splits it into two folders:
  - `llama/avx2/` holds only the variants that need AVX2 (haswell and newer). It is
    the fast path.
  - `llama/compat/` holds only the variants without AVX2 (x64 baseline, SSE4.2,
    Sandy Bridge, Ivy Bridge, Piledriver). It runs on any x86-64 CPU.

  At startup `hardware.rs` checks AVX, AVX2, FMA, F16C and BMI2, which is what the
  haswell variant needs. If all five are present it picks `avx2`, otherwise `compat`.
  If the `avx2` server exits during load, Trurl retries once with `compat` and
  remembers that choice.
- **Arguments** (hardcoded in `ServerConfig`):
  - `-m <file>`
  - `-c <context>`, default 2048
  - `-t <physical cores>`
  - `-np 1`: one slot, which saves RAM.
  - `--cache-ram 0`: the default is 8 GB of prompt cache, wrong for a 4 GB laptop.
  - `--jinja`: the model's own chat template.
  - `--reasoning-format deepseek`
  - `--no-webui`
  - `--host 127.0.0.1 --port <free port> --api-key <random>`
  - `--offline`
- **Lifecycle:**
  - Started when a model is selected.
  - Loading is shown while `/health` returns 503.
  - Ready when it returns 200.
  - Switching model or context stops the server and starts a new one.
  - On exit the server is killed. On Windows it is assigned to a Job Object with
    `KILL_ON_JOB_CLOSE`, so it dies even if Trurl crashes; on Linux it gets
    `PR_SET_PDEATHSIG`.
- **Errors:** the last 200 lines of the server's output are kept and classified into
  plain-language `UserError`s:
  - out of memory
  - not a valid GGUF file
  - unsupported model architecture
  - crashed while chatting
  - context full

## Chat and reasoning

- Requests are OpenAI-style `messages`. The history holds **answers only**: reasoning
  is stored for display and never sent back.
- The stream delivers `delta.reasoning_content` and `delta.content` separately. A
  fallback splitter (`think.rs`) handles models whose template leaves raw
  `<think>…</think>` in `content`, including tags split across chunks.
- The reasoning block has three states:
  - open while thinking;
  - closed when the first answer token arrives;
  - labelled "Thought for 12s", where the duration runs from the first reasoning token
    to the first answer token.
- Stop closes the HTTP stream; llama-server stops generating when the client leaves.
- "Ran out of room" covers both ways the context fills:
  - an HTTP 400 `exceed_context_size_error` before generation;
  - `finish_reason: "length"` during generation.

  Both show: "The conversation ran out of room. Start a new chat or increase the
  context size."
- Temperature 0 is greedy and repeatable (verified: two runs give identical text).

## Models folder

| OS | Path |
|---|---|
| Windows | `%LOCALAPPDATA%\CyberiadLabs\Trurl\models` |
| Linux | `$XDG_DATA_HOME/trurl/models` (default `~/.local/share/trurl/models`) |

- On Windows this folder is outside the install dir (`%LOCALAPPDATA%\Trurl`) and outside
  the Tauri app-data dir, so neither the installer, the updater nor the uninstaller's
  "delete app data" box can reach it. `TRURL_HOME` overrides the location for tests and
  portable use.
- A model is any `*.gguf` file in the folder whose header parses. Multimodal projector
  files (`mmproj*`) are hidden.
- Each model shows its file size and a RAM-fit dot:
  - green when size < 50% of RAM;
  - orange when < 75%;
  - red otherwise.
- Import copies the file into the folder with a progress bar. A file is never moved off
  the USB stick.
- Delete asks first, then removes the file.

## Hugging Face browser and downloads

- **Online check:** `HEAD https://huggingface.co/api/models?limit=1` with a 4 s timeout,
  run only when the browser opens.
- **Search:** `GET /api/models?search=<q>&filter=gguf&sort=downloads&limit=30`.
- **File list:** `GET /api/models/<repo>?blobs=true` gives each GGUF's size and LFS
  SHA-256. The quantization is parsed from the file name.
- **Download:**
  - Data goes to `downloads/<sha256>.part`, with a sidecar `.json` holding the repo,
    file, revision, size and SHA-256.
  - Resuming sends `Range: bytes=<len>-` against the pinned revision, and works across
    app restarts.
  - Pause keeps the `.part` file; cancel deletes it.
  - Once complete, the SHA-256 is checked. Only a matching file is moved into the
    models folder.
- **Token:** optional, kept in settings and sent only to `huggingface.co`.

## Updates

| Front end | How it updates |
|---|---|
| Desktop | Tauri updater plugin. The manifest is `latest.json` on GitHub Releases, and updates are signed with minisign. |
| Linux terminal app | Reads the same `latest.json` (`linux-x86_64` entry), verifies the same signature with `minisign-verify`, and replaces its own folder on `trurl update`. |

- When it checks:
  - once on launch;
  - then every 24 h;
  - and from "Check for updates" in About.
- A failed check is silent.
- "Later" hides the prompt until the next check.
- "Update now" waits for any running generation to finish, stops llama-server, then
  installs.
- The updater replaces the app only. Models live elsewhere (see above).

## Distribution

NSIS installers cannot exceed 2 GB, and Gemma 3 4B alone is 2.49 GB, so the models
cannot live inside the `.exe`. They should not anyway, because the same installer is
what the updater downloads. So:

- **Update installer**, `Trurl_<v>_x64-setup.exe` (~25 MB): installs per user with no
  admin, assumes WebView2 is present.
- **USB package**, built by `scripts/make_usb_bundle.py`:
  - `Trurl-<v>-USB/Trurl_<v>_x64-offline-setup.exe`, which embeds the WebView2 offline
    installer for Windows 10 machines that lack it;
  - `Trurl-<v>-USB/models/*.gguf` plus `LICENSES/`.

  The installer copies every `models/*.gguf` next to it into the models folder,
  skipping files already there. No download is needed on the learner's machine.
- **Linux:** `trurl-<v>-linux-x64.tar.gz` holds the `trurl` binary, both llama builds
  and `install.sh`. The same USB `models/` folder works.

## Testing

| Lane | What | Where |
|---|---|---|
| Gate | Unit tests: think splitter, SSE parser, GGUF reader, RAM fit, quant parser, history building, error mapping, settings, CPU variant choice, download resume against a local Range server that drops connections | `cargo test`, every commit |
| Gate | Copy voice, pins consistency, network allowlist | `scripts/check_*.py` |
| Eval | Real llama-server + Qwen3 0.6B + Gemma 3 1B, all with the network unplugged (`unshare -rn` on Linux): streaming, reasoning split, `/no_think`, temperature 0 repeatability, context overflow message, stop, model switch, crash recovery | `cargo test --test live -- --ignored` |
| Eval | UI flows in Chromium at 1366x768 against the bridge, with screenshots | `apps/bridge/e2e` |
| CI | Windows: build installer, install silently, launch, chat offline (firewall block), uninstall, check models survive | GitHub Actions |
