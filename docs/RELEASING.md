# Publishing an update

Learners get updates through the app. When the computer is online, Trurl checks
GitHub Releases 15 seconds after it starts and then every 24 hours, and shows
"Update available: version X.Y.Z" with "Update now" and "Later". Nothing installs by
itself. This page is how you ship one.

## What learners get

| They run | They download | What changes |
|---|---|---|
| Desktop app (Windows) | `Trurl_<v>_x64-setup.exe` (about 15 MB), checked against the updater key | The app is replaced. Models and settings stay. |
| Terminal app (Linux) | `trurl-<v>-linux-x64.tar.gz`, checked against the same key | The app folder is swapped. Models and settings stay. |

Both read one manifest:
`https://github.com/jbarbier/trurl/releases/latest/download/latest.json`. It only
exists once a release is **published**, not while it is a draft.

## Steps

1. **Bump the version** (it lives in four files; never edit them by hand):
   ```
   python3 scripts/bump_version.py 0.2.0
   ```
2. **Write the release notes** in `CHANGELOG.md`, under `## 0.2.0`. The update prompt
   shows them to learners, so keep them short and plain.
3. **Open a pull request.** CI runs everything:
   - the gates;
   - the offline evals on Linux and Windows;
   - the Windows install smoke test;
   - the Windows self-update test.

   Merge when it is green.
4. **Tag the merge commit** on `main` and push the tag:
   ```
   git tag v0.2.0
   git push origin v0.2.0
   ```
5. **The Release workflow** then does the rest. It:
   - rebuilds llama-server from source;
   - builds both installers and smoke-tests the USB one on Windows;
   - builds the Linux tarball;
   - signs the update files, and checks every signature the way installed copies
     will;
   - writes `latest.json` and creates a **draft** GitHub Release.
6. **Review the draft** on GitHub (Releases), then click **Publish release**. From that
   moment, every Trurl that comes online sees the update.
7. **Refresh your USB sticks** (optional, for computers that are never online):
   `python3 scripts/make_usb_bundle.py`. See [USB-BUNDLE.md](USB-BUNDLE.md).

## The signing key

- **The two keys.** Updates are signed with a minisign key.
  - The **public key** is in `apps/desktop/src-tauri/tauri.conf.json` and in
    `crates/trurl-core/src/update.rs`. A test checks they match.
  - The **private key** and its password are GitHub Actions secrets,
    `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, plus one copy
    kept by the maintainer outside the repo.
- **Who sees the private key.** Only the "sign" step of the Release workflow. No build
  ever sees it.
- **If the private key is lost, installed copies can never be updated again.**
  Learners would have to reinstall by hand. Keep the copy in a password manager.
- **A new key.** For a fork, or after a leak, make a key and store its secrets:
  ```
  (cd apps/desktop && npm ci && npx --no -- tauri signer generate -w ../../trurl-updater.key)
  gh secret set TAURI_SIGNING_PRIVATE_KEY < trurl-updater.key
  gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD      # paste the password you chose
  ```
  Then put the new public key (`trurl-updater.key.pub`) in both files above.
  Copies installed with the old key will not accept updates signed with the new one.

## If something goes wrong

- **The Release workflow failed.**
  - **Nothing to fix in the code** (a network hiccup, a runner problem): delete the
    draft release if one was created, then re-run the failed workflow from the
    Actions tab. It builds the same tagged commit.
  - **The code needed a fix:** a re-run would build the old, tagged commit again.
    Delete the draft release and the tag (`git push --delete origin vX.Y.Z` and
    `git tag -d vX.Y.Z`), merge the fix to `main`, then tag the fixed commit.
- **A bad release was published.** Publish a fixed one with a higher version. There is
  no rollback: Trurl only moves to newer versions.
- **The manifest is missing or broken.** The apps say nothing and keep working.
- **An update fails on a learner's computer** (for example, the connection drops
  during the download). Trurl says so, keeps the old version and its model, and
  offers the update again at the next check.

## Windows code signing (recommended)

The installers are not Authenticode-signed yet, which has two effects:

- **SmartScreen.** It shows "Windows protected your PC" on first install. Click "More
  info", then "Run anyway".
- **Smart App Control.** On Windows 11 computers that have it turned on (most fresh
  installs start in evaluation mode), unsigned programs are blocked and there is no
  per-app way around it.

To fix both:

- get a code-signing certificate: Azure Trusted Signing, or the free SignPath
  Foundation programme for open-source projects;
- add a signing step in `release.yml` for every `.exe`, including the
  `llama-server.exe` builds, before the installers are built.

Updates are protected by the minisign signature either way.
