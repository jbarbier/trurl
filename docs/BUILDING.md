# Building Trurl from source

Everything is pinned (`pins.json`, `Cargo.toml`, `Cargo.lock`,
`rust-toolchain.toml`), so the same commit always builds the same Trurl.

On Windows, type `python` where this page says `python3`. (On a standard Windows
Python install, `python3` opens the Microsoft Store.)

## What you need

| | Windows (desktop app) | Linux (terminal app) |
|---|---|---|
| Rust | install `rustup`; `rust-toolchain.toml` picks the exact version | same |
| C/C++ compiler | Visual Studio 2022 Build Tools, "Desktop development with C++" | `gcc`/`g++` 10 or newer |
| CMake | 3.31.6 (3.14 to 3.31 works) | same, plus `ninja` (optional, faster) |
| Python | 3.10 or newer | same |
| Node.js | 18 or newer: the Tauri CLI runs with `npx` | 18 or newer, only for the UI end-to-end tests |
| Disk | about 6 GB with the models | about 3 GB |

## 1. Build llama-server (once per llama.cpp version)

```
python3 scripts/build_llama.py
```

- **What it does:**
  - Downloads the pinned llama.cpp source and checks its SHA-256.
  - Builds three static `llama-server` executables into `vendor/llama/<os>/{avx2,avx,compat}/`.
- **How long:** 10 to 30 minutes per build on a laptop.
- **What they need:** nothing. There is no DLL, no Visual C++ runtime and no OpenSSL. To prove it:

  ```
  python3 scripts/check_pe_imports.py vendor/llama/linux        # or vendor/llama/windows
  python3 scripts/check_cpu_isa.py vendor/llama/linux           # each build uses only its CPU tier
  python3 scripts/check_cpu_emulated.py --model ~/.cache/trurl/models/stories260K-f32.gguf
                                       # each build answers on an emulated oldest CPU (Intel SDE)
  ```

## 2. Get the models

```
python3 scripts/fetch_models.py --dest ~/.cache/trurl/models
```

All three pinned models, 3.8 GB. Add `--only gemma-3-1b-it,qwen3-0.6b` for the two
small ones (1.3 GB). Downloads resume, and every file is checked against its SHA-256
in `pins.json`.

## 3a. Desktop app (Windows)

```
cd apps/desktop
npm ci                                                     # the Tauri CLI, exactly as package-lock.json pins it
npx --no -- tauri icon ../../branding/app-icon.svg -o src-tauri/icons
npx --no -- tauri dev                                      # run it
npx --no -- tauri build                                    # installer in target/release/bundle/nsis/
npx --no -- tauri build --config src-tauri/tauri.usb.conf.json   # the USB installer (embeds WebView2)
```

The build needs no signing key: updates are signed by the release workflow, after the
build (see [RELEASING.md](RELEASING.md)).

## 3b. Terminal app (Linux)

```
cargo run --release -p trurl-tui                 # run it from the source tree
bash scripts/package_linux.sh                    # dist/trurl-<v>-linux-x64.tar.gz
```

## 3c. The desktop UI in a browser (any OS, for UI work)

```
cargo run -p trurl-bridge          # then open http://127.0.0.1:7861/
```

The bridge serves `apps/desktop/ui` against the real core and the models in your
Trurl models folder. For design work with no model at all, open
`apps/desktop/ui/index.html?mock` in a browser. Scenarios: `&scenario=empty`,
`offline`, `failed`, `update`, `chat`.

## Tests

| Lane | Command | Time |
|---|---|---|
| Gates | `cargo test`, then `python3 scripts/check_pins.py`, `check_copy.py`, `check_network.py` | about 1 minute |
| Evals: real models, network blocked (Linux) | `bash scripts/run_evals.sh` (fetches the models it needs) | about 10 minutes |
| Evals (Windows, run as administrator: it adds firewall rules) | `./scripts/run_evals.ps1` | about 10 minutes |
| UI end to end, with screenshots | `bash apps/bridge/e2e/run.sh` (needs Node.js and the models in `~/.cache/trurl/models`) | about 3 minutes |

What CI runs on every pull request (`.github/workflows/ci.yml`):

| Job | Runs on | What it runs |
|---|---|---|
| Gates | Linux | the gates |
| Linux | Linux | the CPU-tier and import checks, the evals and the UI end to end |
| Windows | Windows | the unit tests, the evals with the firewall blocking the network, then it builds the USB installer and runs `scripts/windows_smoke.ps1` |
| Windows update | Windows | `scripts/windows_update_test.ps1` |

- **`windows_smoke.ps1`:** installs from a USB-style folder, chats offline through the
  real window, uninstalls, and checks the models are kept.
- **`windows_update_test.ps1`:** installs version N and updates it to N+1 through the
  real window.
