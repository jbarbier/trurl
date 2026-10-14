# Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by
[SignPath Foundation](https://signpath.org).

Windows checks who made a program before running it. Trurl's Windows files are signed
so that Windows can tell they come from this project, built from this repository, and
were not changed since.

## What is signed

| File | Signed with | Checked by |
|---|---|---|
| `trurl.exe` (the desktop app) | Authenticode, SignPath Foundation certificate | Windows |
| `Trurl_<version>_x64-setup.exe` (installer) | Authenticode, SignPath Foundation certificate | Windows |
| `Trurl_<version>_x64-offline-setup.exe` (USB installer) | Authenticode, SignPath Foundation certificate | Windows |
| Update files (`.sig` next to the installer and the Linux tarball) | Trurl's own minisign key | Trurl, before it installs an update |

Not signed with the SignPath certificate: `llama-server.exe` and its libraries. They are
[llama.cpp](https://github.com/ggml-org/llama.cpp), built by our release from its pinned
source (see `pins.json`), and SignPath Foundation signs only a project's own code. The
Linux terminal app is not Authenticode-signed (Linux has no such check); its updates are
checked against the minisign key.

Every signed file is built by GitHub Actions from this repository
([`.github/workflows/release.yml`](../.github/workflows/release.yml)). SignPath checks
that the file came from that build before signing it, and a maintainer approves each
signing request by hand. Releases signed this way show **SignPath Foundation** as the
publisher.

## Team roles

| Role | Who |
|---|---|
| Committers (can change the code without another review) | [Julien Barbier (@jbarbier)](https://github.com/jbarbier) |
| Reviewers (review every change from anyone else, such as pull requests) | [Julien Barbier (@jbarbier)](https://github.com/jbarbier) |
| Approvers (approve each signing request) | [Julien Barbier (@jbarbier)](https://github.com/jbarbier) |

All team members use multi-factor authentication on GitHub and on SignPath.

## Privacy

Trurl runs language models on your own computer. Your messages, your settings and your
models stay on it. Trurl has no account, no analytics and no tracking.

**Your chats are saved on this computer only.** Every conversation (your questions, the
answers and the settings used) is kept in Trurl's data folder
(`%LOCALAPPDATA%\CyberiadLabs\Trurl\chats` on Windows, `~/.local/share/trurl/chats` on
Linux) so you can open it again later. It is never sent anywhere. To remove them:
**Chats**, then **Delete** on one chat or **Delete all chats** (in the terminal app: F9,
then D or A). On a shared computer, delete them before you leave.

Trurl connects to the internet only in these cases:

- **Update check (on by default, can be turned off).** Once after it starts and then
  once a day (a check that fails is tried again after 30 minutes, then 1 hour, 2 hours
  and so on, never more often), Trurl downloads
  `https://github.com/jbarbier/trurl/releases/latest/download/latest.json` from GitHub
  to see if a newer version exists. The request carries nothing about you or what you do
  in Trurl; like any web request, it shows your IP address to GitHub. To turn it off:
  **About**, uncheck **Check automatically** (in the terminal app: About, press **A**).
  You can still check by hand. GitHub's privacy statement:
  <https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement>.
- **When you ask for it:** updating Trurl (from GitHub), and searching or downloading
  models in the Hugging Face screen (from huggingface.co). A Hugging Face token, if you
  add one, is sent only to huggingface.co. Hugging Face's privacy policy:
  <https://huggingface.co/privacy>.

Nothing else leaves the computer. The automated tests check this: they run Trurl with
all network access blocked and fail if any request goes out
([`scripts/check_network.py`](../scripts/check_network.py), the offline end-to-end
suite, and the Windows smoke test).
