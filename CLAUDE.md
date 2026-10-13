# CLAUDE.md for Trurl

Rules for anyone (human or agent) changing Trurl. The general working rules (triage,
branching, tests and evals, critic loop, self-rating, safety, voice) are the public
[jbarbier/CLAUDE.md](https://github.com/jbarbier/CLAUDE.md), which maintainers also
carry as `~/.claude/CLAUDE.md`. This file adds what is specific to Trurl. Where the two
conflict, this file wins.

## What Trurl is, in one paragraph

A small, offline-first app to teach how open-weight language models work. Learners run
models on their own low-end laptops (Windows 10/11, 4 to 8 GB RAM, no GPU, often no
internet). The teacher must be able to trust that the app behaves exactly the same in
every class, so **nothing changes unless a maintainer changes it**: every dependency,
binary and model is pinned by version and SHA-256. Two front ends share one Rust core:
a desktop app (Windows, Tauri) and a terminal app (Linux).

## The product rules (a change that breaks one is a bug)

1. **Offline is the normal case.** Starting, loading a model and chatting never touch
   the network. Exactly two features may: the update check and the Hugging Face browser.
   Both are silent and non-blocking when offline. No telemetry, no analytics, no crash
   upload, no fonts or scripts from a CDN. `scripts/check_network.py` holds the
   allowlist of hosts; adding one is a reviewed decision, not a drive-by.
2. **Only the controls a learner needs.** System prompt, temperature, context size,
   model, new chat, stop. Every other knob stays hardcoded in `crates/trurl-core`
   (`ServerConfig`). Adding a control needs a course exercise that requires it.
3. **What the learner sees is what the model got.** Trurl never injects hidden text: no
   extra system prompt, no tools, no environment block. The chat template comes from
   the GGUF file (llama-server `--jinja`). Previous turns' reasoning is never sent back.
4. **Models live outside the install directory** (`%LOCALAPPDATA%\CyberiadLabs\Trurl\models`
   on Windows, `~/.local/share/trurl/models` on Linux). No installer or updater ever
   deletes that folder. The uninstaller deletes it only when the learner ticks
   "Delete the application data" (an explicit choice, to free a small disk).
5. **Plain-language errors.** Every failure a learner can hit maps to a `UserError`
   with a title and a "what to do" line (`crates/trurl-core/src/errors.rs`). A raw
   stack trace or HTTP code on screen is a bug.
6. **1366x768 is the design screen.** Every view must work there without scrolling the
   page sideways, with body text at 16px or more.

## Layout

```
crates/trurl-core/   all logic: llama-server supervisor, chat streaming, reasoning split,
                     models folder, GGUF reader, Hugging Face client, resumable downloads,
                     settings, hardware detection, update manifest. No UI code.
apps/desktop/        Tauri shell (src-tauri/) + vanilla HTML/CSS/JS UI (ui/)
apps/tui/            Linux terminal app (ratatui)
apps/bridge/         dev-only HTTP bridge: serves the desktop UI against the real core in a
                     browser, so the UI is tested and screenshotted on any OS
contracts/           the event and command shapes shared by core and both front ends
pins.json            every pinned version and SHA-256 (single source of truth)
scripts/             fetch pinned deps, gates, USB bundle, release manifest
docs/                user and maintainer docs, README images
```

The core is the contract. Front ends never talk to llama-server or the network directly.

## Gates

```
bash scripts/gates.sh                      # all the fast gates below, stops at the first failure
cargo fmt --all --check                    # formatting, as CI
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked                        # unit + integration (local HTTP servers, no models)
python3 scripts/check_pins.py              # versions agree, vendored hashes, README table, licenses fresh
python3 scripts/check_copy.py              # UI copy voice rules
python3 scripts/check_network.py           # no URL outside the allowlist
python3 scripts/check_cpu_isa.py vendor/llama/linux     # each build uses only its CPU tier
python3 scripts/check_cpu_emulated.py --model <models>/stories260K-f32.gguf  # each build runs on its oldest CPU
python3 scripts/check_pe_imports.py vendor/llama/linux  # no library a learner may lack
bash scripts/run_evals.sh                  # evals: real llama-server + real models, network blocked
bash apps/bridge/e2e/run.sh                # UI flows in Chromium at 1366x768, real core
```

The first four are gates: deterministic, offline, fast. The ISA and import checks need
the llama-server builds (`python3 scripts/build_llama.py`, 10 to 30 minutes per tier
on a laptop). The last two are the eval lane: they fetch the pinned models once
(`scripts/fetch_models.py`, about 1.3 GB), take minutes, and run in CI on every pull
request, on Linux and on Windows (`scripts/run_evals.ps1`, `scripts/windows_smoke.ps1`,
`scripts/windows_update_test.ps1`).

To change the version: `python3 scripts/bump_version.py X.Y.Z` (never by hand).

## Copy and design

- UI copy: second person, short sentences, sentence case. Say "learner", never "student".
  No em dashes. None of the banned words in jbarbier/CLAUDE.md. `check_copy.py` enforces it.
- Visual design follows the Cyberiad Labs design system: tokens in
  `apps/desktop/ui/tokens.css`, generated values only, three radii (12/9/6px), hard
  tinted shadows, one easing curve, system font stack for UI, Sora only for the wordmark.
- The app icon and logo are placeholders in `branding/`. Replace the files, keep the names.

## Pinned versions

Never bump a version as a side effect. A bump is its own PR: update `pins.json`, run
`build_llama.py` and `fetch_models.py` (they re-verify every hash), run the eval lane, update the README table
(`check_pins.py` fails until you do).

## Repo hygiene (public repo)

- No binaries except the README images in `docs/img/` (compressed, each under 300 KB),
  the vendored Sora font (pinned in `pins.json`) and the signature test fixture.
  Models, llama.cpp builds, icons and installers are fetched or generated, never committed.
- No secrets. The updater private key lives in GitHub Actions secrets and in the
  maintainer's password manager, nowhere else.
- No internal notes, scratch files or local paths in the repo. Plans and critic reports
  go to the PR description or `/tmp`.
