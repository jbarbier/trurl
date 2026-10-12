<p align="center">
  <img src="branding/trurl-logo.svg" alt="Trurl" height="72">
</p>

<p align="center">
  <b>Run open-weight language models on your own computer, without internet, and see how they work.</b><br>
  A teaching app by <a href="https://cyberiadlabs.com">Cyberiad Labs</a>. Free and open source (MIT).
</p>

<p align="center">
  <a href="https://github.com/jbarbier/trurl/actions/workflows/ci.yml"><img src="https://github.com/jbarbier/trurl/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
</p>

![Trurl answering with Qwen3 0.6B: the model's thinking shows above the answer, with how long it took](docs/img/chat-thinking.png)

Trurl is made for classrooms where the laptops are old, the memory is small and the
internet is slow or missing. A learner installs it from a USB stick, picks a model,
and talks to it. Everything that matters for understanding a language model is on
the screen, and nothing else:

- **The system prompt**, the instructions the model reads before every message.
- **The temperature**, from 0 (the same answer every time) to 2 (wild).
- **The context size**, how much of the conversation the model can see, and the
  message you get when it runs out of room.
- **The model's thinking**, streamed live in its own block, then folded away with
  "Thought for 12s" when the answer starts.

Nothing is hidden. Trurl adds no secret instructions, no tools, no tracking. The
model gets exactly what the learner sees.

## Contents

- [Install](#install)
- [Using Trurl](#using-trurl)
- [What it runs on](#what-it-runs-on)
- [Benchmarks](#benchmarks)
- [For teachers](#for-teachers)
- [Pinned versions](#pinned-versions)
- [Build, test, release](#build-test-release)
- [How it works](#how-it-works)
- [Licenses](#licenses)

## Install

**From a USB stick (no internet).** Open the stick and double-click
`Trurl_<version>_x64-offline-setup.exe`. The installer copies the models from the
stick's `models` folder. No administrator password is needed. See
[docs/USB-BUNDLE.md](docs/USB-BUNDLE.md) to make the stick.

**From the internet.** Download from the
[latest release](https://github.com/jbarbier/trurl/releases/latest):

| Computer | Download | Then |
|---|---|---|
| Windows 10 or 11 (64-bit) | `Trurl_<version>_x64-setup.exe` | Double-click. Download models from inside Trurl, or import them. |
| Linux (64-bit) | `trurl-<version>-linux-x64.tar.gz` | `tar xzf trurl-*.tar.gz && sh trurl-*/install.sh`, then run `trurl` |

Windows may say "Windows protected your PC" the first time: click **More info**,
then **Run anyway**. (The installer is not code-signed yet. Updates are signed.)

## Using Trurl

### Chat

Type a message and press **Enter**. **Shift+Enter** makes a new line. The answer
appears word by word; **Stop** (or **Esc**) cuts it short. **New chat** (Ctrl+N)
forgets the conversation. Answers are formatted (lists, bold, code), and each one
says which model wrote it, so you can compare models in one conversation.

![Chat](docs/img/chat.png)

### Watch a model think

Reasoning models (Qwen3, DeepSeek-R1 distills and others, detected automatically)
think before they answer. Trurl shows the thinking in a grey block above the
answer, live. When the answer starts, the block folds and says how long the model
thought. Click it to read the thinking again. Type `/no_think` in a message to ask
Qwen3 to skip it.

The thinking is never sent back to the model with the next message: only answers
are, which is how these models are meant to be used.

![Thinking](docs/img/thinking.png)

### Change how the model behaves

On the left:

| Control | What it does | Default |
|---|---|---|
| System prompt | Instructions the model reads before every message. Empty means none. | empty |
| Temperature | 0.0 to 2.0. Low: focused and repeatable (0 gives the same answer every time). High: more varied. | 0.7 |
| Context size | How many tokens of conversation the model can see: 256 to 8,192. Changing it reloads the model. | 2,048 |
| Model | Which model answers. Switching reloads. | Gemma 3 1B |

Each control shows **Reset** when it is not at its default. When a conversation no
longer fits in the context, Trurl says: "The conversation ran out of room. Start a
new chat or increase the context size."

![Controls](docs/img/controls.png)

### Models

Open **Models** to see the models on this computer, with their size and a dot
that says whether they fit in this computer's memory:

| Dot | Meaning |
|---|---|
| 🟢 Fits easily | The file is smaller than half the memory. |
| 🟠 Tight fit | Smaller than three quarters. Close other apps. |
| 🔴 Too big | Trurl asks before loading it. |

**Import model** copies a `.gguf` file from anywhere (a USB stick, a folder) into
Trurl's models folder. **Delete** frees the space.

![Models](docs/img/models.png)

### Download from Hugging Face

When the computer is online, the **Hugging Face** tab searches
[huggingface.co](https://huggingface.co) for models in GGUF format. Click a result
to see its details: who made it, its license, how many parameters it has, its
model card, and every file with its quantization (Q4_K_M, Q8_0…), size and memory
fit. Click **Download** on a file.

Downloads survive bad connections: when the connection drops, Trurl tries again
by itself and continues where it stopped, even after Trurl is closed and opened
again. **Pause**, **Resume** and **Cancel** are always there. Every file is checked
against Hugging Face's SHA-256 before it appears in the model list.

Offline, the tab says "Connect to the internet to browse Hugging Face", and
everything else keeps working.

![Hugging Face](docs/img/huggingface.png)

### Updates

When online, Trurl checks for a new version once at start and once a day. If there
is one, a small card says "Update available: version X.Y.Z" with what changed, and
**Update now** or **Later**. It never updates by itself and never interrupts an
answer. Updates replace the app only: your models stay.

### Keyboard

| Keys | Action |
|---|---|
| Enter / Shift+Enter | send / new line |
| Esc | stop the answer |
| Ctrl+N | new chat |
| Ctrl+Shift+S | hidden settings (Hugging Face token for gated models) |

The Linux terminal version has the same features; press **F1** inside it for its keys.

## What it runs on

| | Desktop app | Terminal app |
|---|---|---|
| Windows 10, 11 (64-bit) | ✅ | ✅ |
| Windows 7, 8.1 (64-bit) | ❌ (Microsoft WebView2 ended support) | not built yet, see [the open item](#for-teachers) |
| Linux, glibc 2.31+ (Ubuntu 20.04+, Debian 11+, Mint 20+) | not shipped | ✅ |

- **CPU:** any 64-bit Intel or AMD. Trurl picks the fastest of three model engines
  at start:
  - **AVX2**: Haswell, 2013, and newer.
  - **AVX**: Sandy Bridge and Ivy Bridge, 2011 to 2012, common in second-hand laptops.
  - **Compatible**: anything older.

  If a computer cannot run the engine Trurl picked, Trurl moves down a level by
  itself.
- **Memory:** 4 GB runs Gemma 3 1B and Qwen3 0.6B. 8 GB also runs Gemma 3 4B.
- **No graphics card needed.**
- **Disk:** about 60 MB for Trurl, plus the models: 0.5 to 2.5 GB each.

## Benchmarks

Measured on the same machine against the other ways to run the same model. See
[docs/BENCHMARKS.md](docs/BENCHMARKS.md) for the method and the raw numbers.

<!-- benchmarks table: filled by scripts/benchmark.py -->

## For teachers

- **Same behaviour in every class.** Trurl never changes unless you install a new
  version: the model engine, the models and every library are pinned (below).
- **Hand it out on USB sticks.** [docs/USB-BUNDLE.md](docs/USB-BUNDLE.md) explains
  how to make one, with the three models, and how to add your own.
- **Course exercises:**
  - Temperature 0 gives the same answer twice (sessions on sampling).
  - Context size 256 makes the conversation run out of room (session 1.3.2). llama.cpp
    rounds any context to a multiple of 256, so 256 is the smallest; it behaves like
    the course's 200.
  - `/no_think` turns Qwen3's thinking off (sessions 1.3.4, 1.4.3).
  - Stop works at any moment, including while the model is thinking.

## Pinned versions

Changing any of these is a deliberate change: a pull request that updates
[`pins.json`](pins.json) and passes the evals. `scripts/check_pins.py` keeps this
table honest.

| Component | Version | Pinned by |
|---|---|---|
| llama.cpp (model engine) | v0.6.0, build b11429, commit d81235049384 | source tarball SHA-256 in `pins.json`; built by `scripts/build_llama.py` |
| Rust | Rust 1.99.0 | `rust-toolchain.toml` |
| Desktop shell | tauri 2.12.3 | `Cargo.toml` (`=` versions) and `Cargo.lock` |
| Terminal UI | ratatui 0.30.2 | `Cargo.toml` and `Cargo.lock` |
| Markdown | marked 18.0.14, dompurify 3.4.16 | vendored files, SHA-256 in `pins.json` |

| Bundled model | File | SHA-256 (start) | Size |
|---|---|---|---|
| Gemma 3 1B Instruct (default) | `gemma-3-1b-it-Q4_K_M.gguf` from `ggml-org/gemma-3-1b-it-GGUF` | `8ccc5cd1f1b36025` | 806 MB |
| Gemma 3 4B Instruct | `gemma-3-4b-it-Q4_K_M.gguf` from `ggml-org/gemma-3-4b-it-GGUF` | `882e8d2db44dc554` | 2.49 GB |
| Qwen3 0.6B | `Qwen_Qwen3-0.6B-Q4_K_M.gguf` from `bartowski/Qwen_Qwen3-0.6B-GGUF` | `9acfc1e001311f34` | 484 MB |

Each model is pinned to a Hugging Face commit, a size and a full SHA-256.

## Build, test, release

- [docs/BUILDING.md](docs/BUILDING.md): build from source, run the tests.
- [docs/RELEASING.md](docs/RELEASING.md): publish an update that every Trurl picks up.
- [docs/USB-BUNDLE.md](docs/USB-BUNDLE.md): make the USB stick, add models to it.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): how Trurl is built, and why.
- [CLAUDE.md](CLAUDE.md): the rules for changing Trurl.

## How it works

Trurl is a small Rust program around [llama.cpp](https://github.com/ggml-org/llama.cpp)'s
`llama-server`:

- **Starting a model.** Trurl starts `llama-server` with the model, listening only
  on this computer (127.0.0.1) and protected by a random key, and stops it when
  Trurl closes, even if Trurl crashes.
- **Low-RAM settings.** One conversation at a time, no prompt cache, threads =
  physical cores, memory-mapped weights.
- **Chat template and reasoning.** The model's own chat template comes from its
  file. Reasoning is separated by llama-server, with a fallback for raw
  `<think>` tags.
- **Two front ends, one core.** The Windows app (Tauri, plain HTML/CSS/JS) and
  the terminal app share one core, so they behave the same.

Details in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Licenses

- **Trurl:** [MIT License](LICENSE), copyright Cyberiad Labs.
- **Models:**
  - Gemma 3: [Gemma Terms of Use](licenses/Gemma-Terms-of-Use.txt). Gemma is provided
    under and subject to the Gemma Terms of Use found at ai.google.dev/gemma/terms.
  - Qwen3: [Apache License 2.0](licenses/Qwen3-Apache-2.0.txt).
- **Software components:** llama.cpp is under the [MIT License](licenses/llama.cpp-MIT.txt).
  Every other component is listed with its license in
  [licenses/THIRD-PARTY.txt](licenses/THIRD-PARTY.txt).
- **In the app:** all of these are also in Trurl's About screen.

The app icon and logo are placeholders until the final artwork is ready.
