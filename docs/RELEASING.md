# Publishing an update

Learners get updates through the app: it checks GitHub Releases once at launch
and every 24 hours, and shows "Update available: version X.Y.Z" with "Update now"
and "Later". Nothing installs by itself. This page is how you ship one.

## What learners get

| They run | They download | What changes |
|---|---|---|
| Desktop app (Windows) | `Trurl_<v>_x64-setup.exe` (about 15 MB), checked against the updater key | The app is replaced. Models and settings stay. |
| Terminal app (Linux) | `trurl-<v>-linux-x64.tar.gz`, checked against the same key | The app folder is swapped. Models and settings stay. |

Both read one manifest:
`https://github.com/jbarbier/trurl/releases/latest/download/latest.json`.

## Steps

1. **Bump the version**, the same number in three places (`scripts/check_pins.py`
   fails until they agree):
   - `pins.json` → `app.version`
   - `Cargo.toml` → `[workspace.package] version`
   - `apps/desktop/src-tauri/tauri.conf.json` → `version`
2. **Write the release notes** in `CHANGELOG.md`, under `## <version>`. The update
   prompt shows them to learners, so keep them short and plain.
3. **Open a PR**. CI runs the gates, the offline evals on Linux and Windows, builds
   the installer and runs the Windows smoke test. Merge when green.
4. **Tag** the merge commit and push the tag:
   ```
   git tag v0.2.0
   git push origin v0.2.0
   ```
5. **Wait for the Release workflow.** It builds both installers and the Linux
   tarball, signs them, writes `latest.json` and publishes the GitHub Release.
   From then on every Trurl that comes online sees the update.
6. **Refresh your USB sticks** (optional, for computers that are never online):
   `python3 scripts/make_usb_bundle.py`. See [USB-BUNDLE.md](USB-BUNDLE.md).

## The signing key

- Updates are signed with a minisign key.
  - The **public key** is in `apps/desktop/src-tauri/tauri.conf.json` and in
    `crates/trurl-core/src/update.rs`, and a test checks they match.
  - The **private key** and its password are GitHub Actions secrets:
    `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
  - A copy is kept by the maintainer, outside the repo.
- **If the private key is lost, installed copies can never be updated again.**
  Learners would have to reinstall by hand. Keep the copy in a password manager.
- Never commit the key, and never print it in CI logs.

## If something goes wrong

- **A bad release**: publish a fixed one with a higher version. There is no
  rollback: Trurl only moves to newer versions.
- **The manifest is missing or broken**: apps say nothing (offline-style silence)
  and keep working. Re-run the Release workflow.
- **An update fails on a learner's computer** (connection drops mid-download): the
  app says so, keeps the old version, and tries again at the next check.

## Windows code signing (optional, recommended)

The installers are not Authenticode-signed yet, so Windows SmartScreen shows
"Windows protected your PC" on first install (click "More info", then "Run
anyway"). To remove that:

- get a code-signing certificate (Azure Trusted Signing, or the free SignPath
  Foundation programme for open-source projects);
- add a signing step after `tauri build` in `release.yml` for every `.exe`,
  including the `llama-server.exe` builds.

Updates are already protected by the minisign signature either way.
