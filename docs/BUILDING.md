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
| Node.js | 22 (the Tauri CLI, from `apps/desktop/package-lock.json`) | 18 or newer, only for the UI end-to-end tests |
| Disk | about 10 GB: build folders, plus 3.8 GB of models | about 8 GB: build folders, plus 3.8 GB of models |

## 1. Build llama-server (once per llama.cpp version)

```
python3 scripts/build_llama.py
```

- **What it does:**
  - Downloads the pinned llama.cpp source and checks its SHA-256.
  - Builds four static `llama-server` executables into `vendor/llama/<os>/{avx2,avx,ssse3,compat}/`.
- **How long:** 10 to 30 minutes per build on a laptop.
- **What they need:** nothing. There is no DLL, no Visual C++ runtime and no OpenSSL.
- **Proving it.** These checks need `objdump` (binutils; on Windows, LLVM's
  `llvm-objdump`, named in `OBJDUMP`) and a tiny test model. Intel's emulator is
  downloaded by the script itself (pinned in `pins.json`, about 40 MB, once).

  ```
  python3 scripts/fetch_models.py --dest ~/.cache/trurl/models --test-models   # stories260K, 1 MB

  # Linux
  python3 scripts/check_pe_imports.py vendor/llama/linux        # no library a learner may lack
  python3 scripts/check_cpu_isa.py vendor/llama/linux           # each build uses only its CPU tier
  python3 scripts/check_cpu_emulated.py vendor/llama/linux --model ~/.cache/trurl/models/stories260K-f32.gguf
                                                                # each build answers on its oldest CPU

  # Windows (PowerShell; <commit> is the first 12 characters of llama_cpp.commit in pins.json)
  python scripts/check_pe_imports.py vendor/llama/windows
  $env:OBJDUMP = "C:\Program Files\LLVM\bin\llvm-objdump.exe"
  python scripts/check_cpu_isa.py --objects "$HOME/.cache/trurl/build-windows-<commit>"
  python scripts/check_cpu_emulated.py vendor/llama/windows --model "$HOME/.cache/trurl/models/stories260K-f32.gguf"
  ```

  On Windows the ISA check reads llama.cpp's own compiled code rather than the
  `.exe`: the static Microsoft C runtime inside it has AVX and FMA versions of a few
  routines that it only uses after asking the CPU. The emulator run is the proof for
  the whole `.exe`.

## 2. Get the models

```
python3 scripts/fetch_models.py --dest ~/.cache/trurl/models
```

All three pinned models, 3.8 GB. Add `--only gemma-3-1b-it,qwen3-0.6b` for the two
small ones (1.3 GB). Downloads resume, and every file is checked against its SHA-256
in `pins.json`.

## 3a. Desktop app (Windows)

Needs Node.js 22, Rust (installed by `rustup` from `rust-toolchain.toml`) and the
Visual Studio Build Tools with the C++ workload.

```
cd apps/desktop
npm ci                                                     # the Tauri CLI, exactly as package-lock.json pins it
npx --no -- tauri icon ../../branding/app-icon.svg -o src-tauri/icons
npx --no -- tauri dev                                      # run it
```

Both installers are written to the same file, `../../target/release/bundle/nsis/Trurl_<v>_x64-setup.exe`
(from `apps/desktop`). Build one, copy it away, then build the other:

```
npx --no -- tauri build -- --locked                        # the small installer (fetches WebView2 if missing)
copy ..\..\target\release\bundle\nsis\Trurl_0.1.0_x64-setup.exe ..\..\dist\
npx --no -- tauri build --config src-tauri/tauri.usb.conf.json -- --locked   # the USB installer (embeds WebView2)
copy ..\..\target\release\bundle\nsis\Trurl_0.1.0_x64-setup.exe ..\..\dist\Trurl_0.1.0_x64-offline-setup.exe
```

`--locked` builds with exactly the crate versions in `Cargo.lock`, as CI and the
release do. Scripts in `scripts/*.ps1` may need
`powershell -ExecutionPolicy Bypass -File scripts\<name>.ps1` on a computer that
blocks local scripts.

The build needs no signing key: updates are signed by the release workflow, after the
build (see [RELEASING.md](RELEASING.md)).

## 3b. Terminal app (Linux)

```
cargo run --release -p trurl-tui                 # run it from the source tree
bash scripts/package_linux.sh                    # dist/trurl-<v>-linux-x64.tar.gz
```

A tarball built on your own distribution needs that distribution's C library (glibc)
or newer. To make one that runs on Ubuntu 20.04, Debian 11 and everything newer,
build inside Debian 11, as the release does:

```
docker run --rm -it -v "$PWD":/src -w /src debian:bullseye bash
# then, inside: the steps of the "linux" job in .github/workflows/release.yml
```

Start with that job's first step. Debian 11 reached end of life in August 2026,
so its packages are now on archive.debian.org only, and `apt-get` fails without it.

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
| Gates | `cargo fmt --all --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`, then `python3 scripts/check_pins.py`, `check_copy.py`, `check_network.py` | about 2 minutes |
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
