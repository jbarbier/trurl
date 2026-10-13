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
pinned commit (`v0.6.0`, build `b11429`), checks its SHA-256, and builds four
**static** `llama-server` executables. Static means one file each: no DLLs, no
Visual C++ runtime, no OpenSSL, no OpenMP.

The official release zips need the Visual C++ runtime on Windows, and glibc 2.34 plus
OpenSSL 3 on Linux. Many classroom computers do not have those.

| Build | Instructions | Runs on |
|---|---|---|
| `avx2` | AVX2, FMA, F16C, BMI2 | Haswell (2013) and newer |
| `avx` | AVX only | Sandy Bridge and Ivy Bridge (2011 to 2012) |
| `ssse3` | SSSE3, no AVX | Core 2 and the first Core i3/i5/i7 (2006 to 2010), Atom, AMD Bobcat |
| `compat` | SSE2 | any 64-bit Intel or AMD CPU (Pentium 4, Athlon 64, AMD Phenom) |

**Why an SSSE3 build.** llama.cpp has fast SSSE3 code for the Q4_0 and Q8_0 formats
and none for the K formats (Q4_K_M). Measured on one computer, Gemma 3 1B with each
engine (words per second while answering / while reading the prompt):

| Engine | Q4_K_M | Q4_0 (QAT) |
|---|---|---|
| compat | 2.1 / 2.6 | 2.8 / 3.5 |
| ssse3 | 2.0 / 2.6 | 8.2 / 16.6 |
| avx | 10.0 / 17.9 | 12.0 / 22.2 |
| avx2 | 12.9 / 32.6 | 13.5 / 35.6 |

(2-core AMD EPYC cloud computer, 2 threads, short prompts, temperature 0, each
engine forced on the same machine.)

So with the ssse3 and compat builds, the Hugging Face screen recommends the Q4_0 file
of a model when there is one (`hub::recommend`), and Q4_K_M otherwise. With AVX both
formats have fast code and Q4_0 gains only a fifth, not worth its lower quality, so
AVX and AVX2 get Q4_K_M, what the course uses. The choice follows the build that
actually runs (after a crash that is a slower one), not the detected processor. The
bundled models stay Q4_K_M, as pinned.

How the build is picked and kept honest:

- **Proven tiers, twice.** `scripts/check_cpu_isa.py` disassembles each build and
  fails if it uses an instruction its CPUs lack: on Linux the whole executable, on
  Windows llama.cpp's own compiled code (the static Microsoft C runtime carries AVX
  and FMA versions of some routines that it only uses after asking the CPU, which a
  disassembler cannot tell apart from a real problem). Then
  `scripts/check_cpu_emulated.py` runs each build under Intel's Software Development
  Emulator pretending to be its oldest CPU (`compat` on a 2004 Pentium 4, `ssse3`
  on a 2006 Core 2 Merom, `avx` on Sandy Bridge, `avx2` on Haswell) and makes it
  answer, twice: with a tiny f32 model and with a Q4_0 copy of it that the script
  writes, so the quantized code paths run too. The emulator stops the program on the
  first instruction that CPU lacks, so a pass means it really runs. The disassembler
  check has a `--self-test` (in the gates) that compiles one sample per instruction
  set and proves each pattern catches it.
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
| `--no-repack` | No second, rearranged copy of the weights in memory. Measured on a 2-core AVX2 computer: Gemma 3 4B uses 2.65 GB instead of 4.1 GB, loads in 2.0 s instead of 4.6 s and answers a little faster; long prompts are read at the same speed (about 45 tokens/s for Gemma 3 1B either way). |
| `--jinja` | The model's own chat template, from the GGUF file. |
| `--reasoning-format deepseek` | Reasoning in its own field. |
| `--no-webui --offline` | Nothing else listens, nothing reaches the network. |
| `--host 127.0.0.1 --port <free port>` | This computer only. |
| `--api-key-file <file>` | A random key, in a file readable only by this user and deleted on stop: command lines are visible to other users. The file is in `run/` beside the models folder (never in it: learners open that folder), passed as a relative path so it stays ASCII. |

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
reasoning token to the first answer token. If the context runs out while the model
is still thinking, the block stays open: the reasoning is all there is to read.
Before the first token the status says "Starting", not "Thinking": only a
reasoning model thinks.

**Stop** closes the HTTP stream, and llama-server stops generating when the client
leaves.

**"The conversation ran out of room. Start a new chat or increase the context size."**
This is the course's message when the context fills (`finish_reason: length`, or
HTTP 400 `exceed_context_size_error`). On the first message of a chat, "Start a new
chat" cannot help, so the hint becomes "Increase the context size, then ask again",
or, when the message alone does not fit, "Your message is too long for the context
size". The desktop app offers one click to the next context size with the question
put back in the box.

**Temperature 0** is greedy and repeatable: the evals check that the same question
gives identical text, chat after chat, with and without a system prompt. At
temperature 0 Trurl asks llama-server not to reuse its prompt cache
(`cache_prompt: false`): a cached prompt is processed in different batches, which
changes the numbers just enough to change a word.

**Each answer keeps its settings** (system prompt, temperature, context), shown
under the model's name, so two answers can be compared.

**Math.** Models write math in LaTeX (`$\frac{1}{2}$`, `\boxed{42}`). Both apps show
it as plain text (1/2, 42) with no math library to pin or download:
`apps/desktop/ui/plainmath.js` and `plain_math` in `apps/tui/src/text.rs`, checked
against the same cases (`contracts/plainmath-cases.json`). Code is never touched.

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

**Next to the models** in the same data folder:

- `settings.json`: the learner's settings and the optional Hugging Face token
  (private to the account on Linux, mode 0600; written to a temporary file and renamed).
- `logs/llama-server.log`: everything the model server printed in this run, and
  `llama-server.previous.log` for the run before, at most 10 MB each. This is the
  file to send when a model will not load.
- `trurl.lock`: one Trurl per data folder. A second copy (another terminal tab)
  says Trurl is already open and exits, instead of loading a second model a
  4 GB laptop cannot hold. The desktop app uses Tauri's single-instance plugin too.
- `downloads/`: unfinished Hugging Face downloads.

**What counts as a model.**

- Any `*.gguf` file whose header parses. Damaged files are listed as such.
- Projector files (`mmproj*`) and the later parts of split models are hidden.
- The header reader (`gguf.rs`) reads only metadata. It takes about 0.3 s for two
  real models. It keeps at most 4 MB of strings from a file and refuses keys over
  64 KB, so a damaged or hostile file cannot fill the memory at startup.
- Only a plain file name in the models folder is ever loaded: a path from anywhere
  else never reaches llama-server.

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
- **Odd server answers.** Nothing on disk changes until the reply is known to be
  the right file: a school proxy that ignores `Range`, or a web filter page sent
  with status 200, never costs the learner the part already downloaded.
  - A 200 (range ignored) is accepted only with exactly the expected size. It is
    written over the part from byte 0, so matching bytes stay as they were.
  - A 200 of any other size (a filter page) is refused and retried; the part is kept.
  - A 206 that starts earlier than asked is written from where it says.
  - A 206 that starts later than asked is refused and retried.
  - A 416 with the size reached means the file is complete; the check decides.
- **Drops.** A dropped connection is retried for as long as Trurl is open, waiting
  2, 5, 10, 20, then 40 seconds between tries. Any progress starts the waits again.
  Pause and Cancel always work.
- **Power cuts.** Every 5 seconds the part file is synced to disk and the job file
  records how many bytes are safe (`synced`). The job file is written to a temporary
  name and renamed, so a cut leaves the old one or the new one, never half of one.
  At the next start the part is cut back to the synced bytes. Part files with no job
  are deleted. Job files are re-checked: an edited one cannot rename the download
  or point outside the models folder.
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

**When it checks:** 15 s after launch (the model loads first), then every 24 hours of
wall-clock time (`update::CheckSchedule`, shared by both apps: a laptop that slept
through the 24 hours checks within 10 minutes of waking), and when the learner
clicks "Check for updates" in About. Never while an answer is being written.
Offline or failed checks are silent.

**"Later"** hides the offer until the next launch or the next 24-hour check.

**"Update now":**

- It is disabled while an answer is being written.
- It downloads first, with a progress bar, while Trurl keeps working. No data for a
  minute fails the download cleanly (the next "Update now" starts again); there is
  no limit on the whole download, so a slow link still gets there.
- If an answer started meanwhile, it waits, and keeps the downloaded bytes.
- Then it stops only the model server and installs. `App::release_engine` checks
  that no answer is running and blocks new ones in one step, so no answer can start
  in between and be cut off. If installing fails, the model comes back and Trurl
  carries on as it was.

**Never touched:** updates replace the app only. The models and settings live in the
data folder.

**The signing key** never touches a build. Builds do not sign
(`createUpdaterArtifacts` is off). The release workflow signs in one step of the
`publish` job, with `tauri signer sign --app-version` from the locked Tauri CLI,
installed before the key enters the environment. Only that step sees the key. A
later step, which never sees it, checks every signature with the same code the apps
use (`examples/verify_update.rs`).

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
- **Linux:** `trurl-<v>-linux-x64.tar.gz` holds the `trurl` binary, the four
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
