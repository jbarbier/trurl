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
   - builds both installers, has them code-signed (you approve two requests on
     signpath.io, see below) and smoke-tests the USB one on Windows;
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

## Windows code signing (SignPath Foundation)

Without an Authenticode signature, Windows SmartScreen shows "Windows protected your
PC" and many learners stop there. The Release workflow signs the Windows files through
[SignPath Foundation](https://signpath.org), free for open-source projects. The policy
learners and SignPath read is [CODE-SIGNING.md](CODE-SIGNING.md).

### How a signed release runs

1. Windows job: `tauri build --no-bundle`, then `scripts/patch_bundle_type.py` marks
   `trurl.exe` as installed by NSIS (what `tauri bundle` would do, and it must happen
   before signing, since changing a signed file breaks the signature).
2. `trurl.exe` is uploaded as a GitHub artifact and submitted to SignPath
   (configuration `trurl-exe`). **You approve the request on signpath.io.**
3. The signed `trurl.exe` goes into both installers (`tauri bundle --no-binary-patching`).
4. Both installers are submitted together (configuration `installers`). **You approve
   again.**
5. The smoke test installs the signed USB installer and checks both signatures, then
   the publish job adds the updater signatures (over the signed files) as before.

Each request waits up to 2 hours for approval; after that the run fails and you re-run
it from the Actions tab. SignPath emails the approver when a request is waiting.

### One-time setup

1. **Apply** at <https://signpath.org/apply> for `https://github.com/jbarbier/trurl`.
   Turn on two-factor authentication on GitHub first (required). Mention that:
   - `llama-server.exe` (llama.cpp, MIT) is built from pinned upstream source and
     shipped unsigned inside the installer; only Trurl's own `trurl.exe` and the
     installers are signed;
   - the USB installer embeds Microsoft's WebView2 runtime installer (Microsoft-signed,
     redistributable), which the app needs to draw its window;
   - the only automatic network contact is the update check, documented in
     CODE-SIGNING.md and off with one checkbox.
2. **In SignPath**, once approved:
   - Project slug `trurl`, repository `https://github.com/jbarbier/trurl`.
   - Trusted build system: GitHub.com, linked to the project.
   - Artifact configurations: `trurl-exe` (paste
     [`packaging/signpath/trurl-exe.xml`](../packaging/signpath/trurl-exe.xml)) and
     `installers` (paste [`packaging/signpath/installers.xml`](../packaging/signpath/installers.xml)).
   - Signing policy slug `release-signing`, with the SignPath Foundation certificate,
     manual approval, approver: you.
   - An API token for a user with the **submitter** role on that policy.
3. **In GitHub** (Settings, Secrets and variables, Actions):
   ```
   gh secret set SIGNPATH_API_TOKEN                     # paste the token
   gh variable set SIGNPATH_ORGANIZATION_ID --body <your SignPath organization id>
   ```
   Signing is on from the next run. Without the variable, releases build unsigned and
   the run shows a warning.
4. **Try it without releasing:** Actions, Release, **Run workflow**, tick "Code-sign".
   A dry run builds, signs and smoke-tests everything, and creates no release.

### What signing does not cover

- **SmartScreen reputation.** A signature says who made the file. SmartScreen may still
  warn for a while on a brand-new release until enough people have run it; SignPath
  Foundation's certificate is shared by many projects and has a long history, which
  usually makes this short.
- **Smart App Control.** On Windows 11 computers that have it on, every program must be
  signed or known to Microsoft. `llama-server.exe` is not signed (SignPath Foundation
  only signs a project's own code), so on those computers Trurl most likely installs
  but cannot start a model. Not tested yet: it needs a Windows 11 computer with Smart
  App Control on. Fixing it needs either llama.cpp's maintainers to sign their builds,
  or Trurl's own certificate (Azure Artifact Signing) to sign them.

Updates are protected by the minisign signature either way.
