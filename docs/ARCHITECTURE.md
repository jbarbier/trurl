# Trurl architecture

How Trurl is built and why. For using Trurl, see the [README](../README.md).

## Goals, in priority order

1. **Same behaviour in every class.** Every binary, library and model is pinned by
   version and SHA-256 in [`pins.json`](../pins.json). Nothing changes on a learner's
   computer unless someone clicks "Update now".
2. **Works offline on a 4 GB laptop.** No network on the chat path. Low-RAM defaults.
3. **Teaches, does not hide.** The learner sees what the model sees: no injected
   prompts, the model's own chat template, reasoning shown apart from the answer.
4. **Small and simple.** Only the controls the course needs.

## The pieces

```
                 ┌──────────────────────── trurl-core (Rust library) ───────────────────────┐
 apps/desktop ──▶│ App           one object: settings, models, engine, chat, downloads     │
 (Tauri + HTML)  │ Engine        starts and watches llama-server (one model at a time)     │
                 │ Chat          streams /v1/chat/completions, splits reasoning from answer │
 apps/tui ──────▶│ Models        models folder, GGUF headers, size, RAM fit, SHA-256 checks │
 (ratatui)       │ Hub           Hugging Face search and model details (only when online)  │
                 │ Downloads     resumable HTTP Range downloads, SHA-256 verified          │
 apps/bridge ───▶│ Update        latest.json + minisign checks (terminal app self-update)  │
 (dev only)      │ Hardware      RAM, physical cores, CPU features → llama-server build    │
                 └──────────────────────────────┬───────────────────────────────────────────┘
                                                │ 127.0.0.1:<random port>, random API key
                                         llama-server (pinned llama.cpp, built by us)
```

- **One API.** Both front ends call the same `trurl_core::app::App`. Desktop
  commands go through one function, `trurl_core::api::dispatch`, and both front ends
  receive the same `Event` stream ([contracts/events.md](../contracts/events.md)).
- **No framework.** The desktop UI is plain HTML, CSS and JavaScript, with no
  framework and no bundler. Its only libraries are two vendored, pinned files for
  Markdown (`marked`, `DOMPurify`).
- **A test bridge.** `apps/bridge` serves the same UI against the real core over
  HTTP, so the UI is tested end to end in a browser on any OS.

## llama-server

**Built from source.** `scripts/build_llama.py` downloads the llama.cpp source at the
pinned commit (`v0.6.0`, build `b11429`), checks its SHA-256, and builds three
**static** `llama-server` executables. Static means one file each: no DLLs, no
Visual C++ runtime, no OpenSSL, no OpenMP.

The official release zips need the Visual C++ runtime on Windows, and glibc 2.34 plus
OpenSSL 3 on Linux. Many classroom computers do not have those.

| Build | Instructions | Runs on |
|---|---|---|
| `avx2` | AVX2, FMA, F16C, BMI2 | Haswell (2013) and newer |
| `avx` | AVX only | Sandy Bridge and Ivy Bridge (2011 to 2012) |
| `compat` | SSE2 | any 64-bit Intel or AMD CPU |

How the build is picked and kept honest:

- **Proven tiers.** `scripts/check_cpu_isa.py` disassembles each build and fails if
  a build uses an instruction its CPUs lack. `compat` contains zero AVX instructions.
- **Picked at start.** `hardware.rs` reads the CPU features (`is_x86_feature_detected!`,
  which also checks that the OS supports AVX) and picks the fastest build present.
- **Stepping down.** If a build still crashes with an illegal-instruction error,
  Trurl drops one tier, remembers it (`settings.max_variant`) and retries. Any other
  failure never lowers the tier.

**Arguments** (all in `ServerConfig`, `crates/trurl-core/src/server.rs`):

| Argument | Why |
|---|---|
| `-m <file>` | Relative to the models folder, which is the working directory, so non-ASCII Windows user names never reach the command line. |
| `-c <context>` | 2048 by default, the learner's choice of 256 to 8,192. |
| `-t <physical cores>` | Hyper-threads slow token generation down. |
| `-np 1` | One conversation, so no spare KV cache. |
| `--cache-ram 0` | The default prompt cache is 8 GB, wrong for a 4 GB laptop. |
| `--fit off` | Trurl sets the context; llama.cpp must not change it. |
| `--jinja` | The model's own chat template, from the GGUF file. |
| `--reasoning-format deepseek` | Reasoning in its own field. |
| `--no-webui --offline` | Nothing else listens, nothing reaches the network. |
| `--host 127.0.0.1 --port <free port>` | This computer only. |
| `--api-key-file <file>` | A random key, in a file readable only by this user and deleted on stop: command lines are visible to other users. |

**Sampling** is llama.cpp's default, plus the learner's temperature. We measured
Qwen3 0.6B on 9 thinking questions at the 2,048 context:

| Settings | Median thinking | Ran out of room |
|---|---|---|
| llama.cpp default (top_k 40, min_p 0.05) | 1,009 tokens | 0 of 9 |
| Qwen's recommended (top_k 20, min_p 0) | 774 tokens | 2 of 9 |
| Qwen's recommended + presence penalty 1.5 | 1,100 tokens | 1 of 9 |

So the defaults stay.

**Lifecycle:**

1. Started when a model is selected. The UI shows Loading while `/health` refuses
   or returns 503. No time limit: a 4B model on a slow disk can take minutes.
2. Switching model or context stops the server, waits for it to exit, then starts the
   new one.
3. A server that dies while idle is noticed within 2 s; one that dies mid-answer is
   noticed by the broken stream. Either way the learner sees "The model stopped
   working" and Trurl reloads it.
4. It never outlives Trurl:
   - **Windows:** it runs in a Job Object with `KILL_ON_JOB_CLOSE`, so it dies even
     if Trurl crashes.
   - **Linux:** it gets `PR_SET_PDEATHSIG`, set from one long-lived thread, because
     the signal follows the thread that started the process.

**Errors.** The last 300 lines of the server's output are kept. When the server
exits, they are read to tell apart:

- the model is too big for this computer;
- the file is damaged;
- the architecture is not supported;
- another program took the port (Trurl retries);
- the CPU lacks an instruction (Trurl steps down a build).

The learner sees each as a plain sentence with what to do next (`errors.rs`).

## Chat and reasoning

**History.** What goes back to the model is the system prompt, then each finished
question and answer:

- Reasoning is shown on screen but never sent back.
- A turn with no answer (stopped before the first word, or failed) is left out
  together with its question. The history must alternate user and assistant:
  Gemma's template refuses anything else.

**Reasoning.** It arrives in `reasoning_content`. For templates that leave raw
`<think>…</think>` tags in the answer, a fallback splitter (`think.rs`) separates
them, including tags cut across two chunks. If only a closing tag appears (the
prompt opened the thought), the text so far is moved into the reasoning block.

**The thinking block.** It is open while the model thinks, and closes when the first
answer word arrives, with "Thought for 12s". That is the time from the first
reasoning token to the first answer token.

**Stop** closes the HTTP stream, and llama-server stops generating when the client
leaves.

**"The conversation ran out of room. Start a new chat or increase the context size."**
This message covers both ways the context fills:

- the question is too long to start (HTTP 400 `exceed_context_size_error`);
- the context fills while answering (`finish_reason: length`).

**Temperature 0** is greedy and repeatable: the evals check that two runs give
identical text.

## Models folder

| OS | Path |
|---|---|
| Windows | `%LOCALAPPDATA%\CyberiadLabs\Trurl\models` |
| Linux | `$XDG_DATA_HOME/trurl/models` (default `~/.local/share/trurl/models`) |

**Out of reach of installs and updates.** On Windows the install folder is
`%LOCALAPPDATA%\Trurl`, and Tauri's own app-data folder is
`%LOCALAPPDATA%\com.cyberiadlabs.trurl`. The models folder is neither:

- the installer, the updater and Tauri's "delete app data" step never reach it;
- `scripts/check_pins.py` stops the names that make this true from drifting;
- the uninstaller removes the models only if the learner ticks "Delete the application
  data".

`TRURL_HOME` moves everything, for tests and portable use.

**What counts as a model.**

- Any `*.gguf` file whose header parses. Damaged files are listed as such.
- Projector files (`mmproj*`) and the later parts of split models are hidden.
- The header reader (`gguf.rs`) reads only metadata. It takes about 0.3 s for two
  real models.

**RAM fit**, shown as a dot next to every model, compares the file size with the
computer's memory:

| Dot | File size |
|---|---|
| Green | under 50% of RAM |
| Orange | under 75% of RAM |
| Red | 75% or more. Trurl asks before loading it. |

**Import.** The file is copied in with progress, to `.part` first and renamed when
complete, after checking free space. A file is never moved off a USB stick.

**Bundled models** are checked against their SHA-256 once, in the background. The
result is cached by size and date.

## Hugging Face and downloads

**Online check.** `GET https://huggingface.co/api/models?limit=1` with a 5 s timeout,
and the answer must be JSON, so a hotel or hotspot login page counts as offline. It
runs only when the learner opens the Hugging Face tab.

**Search and details.**

- **Search:** `GET /api/models?search=<q>&filter=gguf&sort=downloads&limit=30`.
- **Details:** `GET /api/models/<repo>?blobs=true` gives every GGUF file with its size
  and SHA-256, plus the license, parameter count and architecture. The model card
  comes from `/<repo>/raw/<commit>/README.md`.
- **Pinned commit:** downloads use the exact commit SHA shown.

**Downloads** (`downloads.rs`) are built for connections that drop every few minutes:

- **Where the bytes go.** Bytes go to `downloads/<id>.part`, and the job to
  `downloads/<id>.json`.
- **Resuming.** Every attempt asks `huggingface.co/<repo>/resolve/<commit>/<file>`
  again with `Range: bytes=<have>-`. The signed CDN URL it redirects to is never
  stored, because it expires.
- **Odd server answers.**
  - A 200 (range ignored) restarts the download.
  - A 206 that starts earlier than asked is written from where it says.
  - A 206 that starts later than asked restarts the download.
  - A 416 means the file is complete.
- **Drops.** A dropped connection is retried with backoff. Any progress resets the
  retry budget.
- **Pause and restart.** Pause keeps the bytes. Downloads cut by closing Trurl carry
  on at the next start; the ones the learner paused stay paused.
- **One writer.** One task at a time writes a file (a quick Pause then Resume waits
  for the old task).
- **Finishing.** A finished file must match Hugging Face's SHA-256 and parse as GGUF.
  It then takes a name nobody else holds, claimed atomically, in the models folder.
- **The token** is optional, kept in settings, sent only to `huggingface.co`.
  reqwest drops it on redirects to other hosts.

## Updates

| Front end | How it updates |
|---|---|
| Desktop (Windows) | Tauri updater plugin. It reads `latest.json` from GitHub Releases (`windows-x86_64`). The minisign signature must also name the announced version (`requireSignedVersion`). |
| Terminal app (Linux) | Reads the same `latest.json` (`linux-x86_64`) and checks the same key and the same signed version. It only replaces a folder that carries the `.trurl-app` marker. |

**When it checks:** 15 s after launch (the model loads first), then every 24 hours,
and when the learner clicks "Check for updates" in About. Offline or failed checks
are silent.

**"Later"** hides the offer until the next check.

**"Update now":**

- It is disabled while an answer is being written.
- It downloads first, with a progress bar, while Trurl keeps working.
- If an answer started meanwhile, it waits.
- Then it stops only the model server and installs. If installing fails, the model
  comes back and Trurl carries on as it was.

**Never touched:** updates replace the app only. The models and settings live in the
data folder.

**The signing key** never touches a build. Builds do not sign
(`createUpdaterArtifacts` is off). The release workflow signs in one step of a job
that builds no project code, with `tauri signer sign --app-version`. Then it checks
every signature with the same code the apps use (`examples/verify_update.rs`).

## Distribution

**Why a folder.** NSIS installers cannot exceed 2 GB, and Gemma 3 4B alone is 2.49 GB,
so the models cannot live inside the `.exe`. They should not anyway: the same
installer is what the updater downloads, and learners must not download 3.8 GB of
models for an update.

- **Update installer**, `Trurl_<v>_x64-setup.exe` (12 MB): installs per user, no
  admin. It downloads WebView2 only if Windows lacks it.
- **USB package**, built by `scripts/make_usb_bundle.py`:
  - `Trurl_<v>_x64-offline-setup.exe`, which embeds the WebView2 offline installer;
  - `models/*.gguf`, which the installer copies to the models folder (`.part` then
    rename, same-size files skipped; never during an update);
  - `LICENSES/`, `READ-ME-FIRST.txt` and `SHA256SUMS.txt`.
- **Linux:** `trurl-<v>-linux-x64.tar.gz` holds the `trurl` binary, the three
  llama-server builds, the licenses and `install.sh`. It is built in Debian 11 so it
  runs on glibc 2.31 and newer.

## Testing

| Lane | What | Where |
|---|---|---|
| Gate | Unit tests: reasoning splitter, SSE parser, GGUF reader (hostile files too), RAM fit, quant names, history, errors, settings, CPU tier choice, model preselection, signatures, the terminal app's text and update logic | `cargo test` |
| Gate | Downloads against a local server that drops connections, ignores ranges, answers the wrong range, wants a token, serves a damaged file; restart and rapid pause/resume | `crates/trurl-core/tests/downloads.rs` |
| Gate | Pins, copy voice, network allowlist, no DLL dependency, CPU instruction tiers | `scripts/check_*.py` |
| Eval | The real core, real llama-server, real models, with every attempt to reach the network blocked and reported (`scripts/netblock.c` on Linux, Windows Firewall on Windows) | `scripts/run_evals.sh`, `run_evals.ps1` |
| Eval | The real UI in Chromium against the real core: every learner flow, a real Hugging Face download, the offline tab | `apps/bridge/e2e/run.sh` |
| CI | Windows: build the USB installer, install it silently from a USB-style folder, start it offline, chat through the real window, close, uninstall, models kept | `scripts/windows_smoke.ps1` |
| CI | Windows: install version N, update it to N+1 through the real window, check N+1 runs and the models are kept | `scripts/windows_update_test.ps1` |
