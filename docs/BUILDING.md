# Building Trurl from source

Everything is pinned (`pins.json`, `Cargo.toml`, `Cargo.lock`,
`rust-toolchain.toml`), so the same commit always builds the same Trurl.

## What you need

| | Windows (desktop app) | Linux (terminal app) |
|---|---|---|
| Rust | installed by `rustup`; `rust-toolchain.toml` picks the exact version | same |
| C/C++ compiler | Visual Studio 2022 Build Tools ("Desktop development with C++") | `gcc`/`g++` 10 or newer |
| CMake | 3.31.6 (any 3.14 to 3.31 works) | same |
| Python | 3.10 or newer | same |
| Node.js | 18 or newer (only to run the Tauri CLI with `npx`) | not needed |
| Disk | about 6 GB with the models | about 3 GB |

## 1. Build llama-server (once per llama.cpp version)

```
python3 scripts/build_llama.py
```

- **What it does:**
  - Downloads the pinned llama.cpp source and checks its SHA-256.
  - Builds three static `llama-server` executables into `vendor/llama/<os>/{avx2,avx,compat}/`.
- **How long:** 10 to 30 minutes per variant on a laptop.
- **What the variants need:** nothing. There is no DLL, no Visual C++ runtime and no OpenSSL. To prove it:

  ```
  python3 scripts/check_pe_imports.py vendor/llama/windows
  ```

## 2. Get the models (for testing, and for the USB package)

```
python3 scripts/fetch_models.py --dest ~/.cache/trurl/models
```

Downloads are resumable, and every file is checked against its SHA-256 in
`pins.json`.

## 3a. Desktop app (Windows)

```
npx @tauri-apps/cli@2.12.2 icon branding/app-icon.svg -o apps/desktop/src-tauri/icons
cd apps/desktop
npx @tauri-apps/cli@2.12.2 dev        # run it
npx @tauri-apps/cli@2.12.2 build      # NSIS installer in target/release/bundle/nsis/
```

`build` signs the updater artifacts, so it needs `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` in the environment. For a local build without
the key, set `"createUpdaterArtifacts": false` in a config override:
`npx @tauri-apps/cli@2.12.2 build --config '{"bundle":{"createUpdaterArtifacts":false}}'`.

## 3b. Terminal app (Linux)

```
cargo run --release -p trurl-tui                 # run from the source tree
bash scripts/package_linux.sh                    # dist/trurl-<v>-linux-x64.tar.gz
```

## 3c. The desktop UI in a browser (any OS, for UI work)

```
cargo run -p trurl-bridge          # then open http://127.0.0.1:7861/
```

The bridge serves `apps/desktop/ui` against the real core and real models. For
pure design work with no model at all, open
`apps/desktop/ui/index.html?mock` in a browser (scenarios: `&scenario=empty`,
`offline`, `failed`, `update`, `chat`).

## Tests

| Lane | Command | Time |
|---|---|---|
| Gates | `cargo test`, then `python3 scripts/check_pins.py`, `scripts/check_copy.py`, `scripts/check_network.py` | about 1 minute |
| Evals: real models, network blocked | `bash scripts/run_evals.sh` (Linux) or `./scripts/run_evals.ps1` (Windows) | about 10 minutes |
| UI end to end, with screenshots | `bash apps/bridge/e2e/run.sh` | about 3 minutes |

CI runs all of them on Linux and Windows for every push.
