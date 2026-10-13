# The USB package

Classrooms often have no internet, so Trurl is handed out on USB sticks: one
installer plus the models. Nothing has to be downloaded on the learners' computers.
One person makes the stick once, on a computer with internet (about 4.8 GB to
download), then copies it to as many sticks as needed.

## What is on the stick

```
Trurl-0.1.0-USB/
├── Trurl_0.1.0_x64-offline-setup.exe   ← learners double-click this
├── models/                             ← the AI models (GGUF files)
│   ├── gemma-3-1b-it-Q4_K_M.gguf      806 MB   course
│   ├── gemma-3-1b-pt.Q4_K_M.gguf      806 MB   course (optional exercise, 1.1.2)
│   ├── Qwen_Qwen3-0.6B-Q4_K_M.gguf    484 MB   course
│   └── gemma-3-4b-it-Q4_K_M.gguf      2.5 GB
├── linux/                              (only with --linux)
├── LICENSES/                           Gemma Terms of Use and every other license
├── READ-ME-FIRST.txt                   instructions for whoever holds the stick
└── SHA256SUMS.txt                      to check that a copy is complete
```

**Why a folder and not one big installer?** A Windows installer cannot be bigger than
2 GB, and the Gemma 3 4B model alone is 2.5 GB. The installer is also what Trurl
downloads when it updates itself, and nobody should download 4.6 GB of models for an
update.

**What the installer does with the `models` folder.** It copies every model file
from the `models` folder next to it into the learner's models folder.

- **No half-copied files.** Each file is copied under a temporary name and renamed
  only when it is complete. Pulling the stick out midway never leaves a broken model
  that looks whole.
- **No repeat copies.** Files already on the computer with the same size are skipped,
  so installing a second time is quick.
- **A check on first start.** Trurl compares each included model with its known
  SHA-256 fingerprint and marks a damaged copy as "Damaged file".

The `offline-setup` installer also contains Microsoft WebView2 (what draws Trurl's
window), for Windows 10 computers that do not have it yet. Windows 11 always has it.

## Make the folder

### Before you start

1. **A copy of this repository.** On GitHub, click **Code**, then **Download ZIP**.
   Unzip it, and open a terminal (PowerShell on Windows) in that folder.
2. **Python 3.10 or newer.** The command is `python` on Windows and `python3` on Linux.
3. **About 9.4 GB of free disk:** 4.6 GB for the models (kept in a cache folder for
   next time) plus 4.8 GB for the finished folder.
4. **Internet for the first run:** about 4.8 GB to download, once.
5. **Sticks of 8 GB or more** for the full folder (4.8 GB). For 4 GB sticks, leave out
   Gemma 3 4B (see below). FAT32 and exFAT sticks both work, since no file is over 4 GB.

### Build it

```
python scripts/make_usb_bundle.py                # Windows
python3 scripts/make_usb_bundle.py               # Linux
python3 scripts/make_usb_bundle.py --linux       # also the Linux terminal version
```

- **The installer** is downloaded from GitHub Releases, for the version written in
  `pins.json` of your copy of the repository, and checked against the release's
  `SHA256SUMS.txt`. That release must be published.
- **Before the first release**, take the installer that CI built: on GitHub, open
  **Actions**, then the latest green **CI** run, then download the
  **windows-installer-and-screenshots** artifact. Unzip it; the installer is
  `usb/Trurl_<version>_x64-offline-setup.exe`. Then:
  ```
  python scripts/make_usb_bundle.py --installer path\to\Trurl_0.1.0_x64-offline-setup.exe
  ```
- **A stick for Linux computers only**, with a terminal version you built yourself
  (see [BUILDING.md](BUILDING.md), "Terminal app"):
  ```
  python3 scripts/make_usb_bundle.py --no-windows --linux-tarball dist/trurl-0.1.0-linux-x64.tar.gz
  ```
- **The models** are downloaded once into a cache folder (`~/.cache/trurl/models`),
  with each file checked against its SHA-256 fingerprint, then copied into the folder.
- **The result** is `dist/Trurl-<version>-USB/`. Copy the whole folder onto each stick.

### Check a copied stick

On Linux, in the stick folder:

```
sha256sum -c SHA256SUMS.txt
```

On Windows, in PowerShell in the stick folder (no output means every file is good):

```
Get-Content SHA256SUMS.txt | % { $h,$f = $_ -split '  ',2; if ((Get-FileHash $f).Hash -ne $h) { "BAD: $f" } }
```

## Choose the models

- **A smaller stick** with only the three course models (2.1 GB), for 4 GB sticks or
  4 GB laptops:
  ```
  python3 scripts/make_usb_bundle.py --only gemma-3-1b-it,gemma-3-1b-pt,qwen3-0.6b
  ```
  The installer copies every model on the stick to each computer. To spare small
  disks the optional Gemma 3 1B PT (806 MB), leave it out too:
  `--only gemma-3-1b-it,qwen3-0.6b` (1.3 GB). Learners who want it later download it
  in one click under **For the course**.
- **One more model**, from any GGUF chat model file you have:
  ```
  python3 scripts/make_usb_bundle.py --extra-model path/to/model.gguf
  ```
  Or just copy the `.gguf` file into the stick's `models/` folder.
### A stick for 2006 to 2010 laptops

Core 2, first Core i3/i5/i7 and Atom laptops run Trurl's SSSE3 engine. There, the
Q4_K_M files above are slow (2 to 4 tokens per second) and the Q4_0 versions of the
same small models are two to four times faster, and read a message about five times
faster (measurements in [ARCHITECTURE.md](ARCHITECTURE.md#llama-server), "Why an SSSE3 build"). Trurl's Hugging Face screen
recommends them on those laptops, but a class without internet needs them on the
stick:

1. Download these two files in a browser (no account needed):
   - [gemma-3-1b-it-qat-Q4_0.gguf](https://huggingface.co/ggml-org/gemma-3-1b-it-qat-GGUF/resolve/c2531de7eec944fecd05bb2c23804c50d5a915c4/gemma-3-1b-it-qat-Q4_0.gguf),
     720 MB, SHA-256 `ef60e4e91a738c99ae9976b050657dfe68a4007a0ccca121b55ec0c413dccd58`
     (Google's Gemma 3 1B, trained to be stored as Q4_0; Gemma Terms of Use, already in `LICENSES/`)
   - [Qwen_Qwen3-0.6B-Q4_0.gguf](https://huggingface.co/bartowski/Qwen_Qwen3-0.6B-GGUF/resolve/60b85c0e3d8fe0f6474f406922a26d12aca4550d/Qwen_Qwen3-0.6B-Q4_0.gguf),
     470 MB, SHA-256 `4b78d8e3c61976cebb78ef5affe19d0eca75b1b47ec66f613a5a3245484758d5`
     (Apache 2.0)
2. Check each one: `sha256sum <file>` on Linux, `Get-FileHash <file>` in PowerShell.
   The result must match the line above.
3. Build the stick with them, next to the two small Q4_K_M models (or leave those
   out by adding the files to `models/` of an existing stick by hand):
   ```
   python3 scripts/make_usb_bundle.py --only gemma-3-1b-it,qwen3-0.6b --extra-model ~/Downloads/gemma-3-1b-it-qat-Q4_0.gguf --extra-model ~/Downloads/Qwen_Qwen3-0.6B-Q4_0.gguf
   ```

On these laptops, pick the Q4_0 model in Trurl's model list. Its answers can differ
a little from the Q4_K_M file's, as between any two files of the same model.

### Pinned models

- **A model in every Trurl release** (pinned, checked by fingerprint, listed in the
  About screen with its license): add it to `models` in `pins.json` with its Hugging
  Face repository, commit, file name, size and SHA-256. Then add its license to
  `licenses/`.

## On the learner's computer

**Windows 11 and Smart App Control.** Until Trurl's installer is code-signed (see
[RELEASING.md](RELEASING.md)), two Windows protections react to it:

- **SmartScreen** shows "Windows protected your PC". Click **More info**, then
  **Run anyway**. This is normal for a new, unsigned program.
- **Smart App Control** (some new Windows 11 computers) blocks it with no way to run
  it anyway. It can only be turned off in **Windows Security**, **App & browser
  control**, **Smart App Control settings**, and Windows does not let you turn it
  back on without reinstalling. Decide this with the computer's owner. Signing
  the installer removes the problem for everyone.

### Models on the computer

- **Where the models go:** `%LOCALAPPDATA%\CyberiadLabs\Trurl\models` on Windows.
- **Updates never touch them.** Uninstalling keeps them too, unless the learner ticks
  "Delete the application data" in the uninstaller.
- **Adding a model later** from a stick needs no reinstall: in Trurl, open
  **Models**, then **Import model**.

## Update a class

Learners' computers that go online get the update card by themselves. For a class
with no internet:

1. On the online computer, download the new copy of this repository (its `pins.json`
   names the new version) and build the folder again. The models are already in the
   cache, so only the installer is downloaded.
2. On each laptop, run the new `offline-setup` from the stick, over the old Trurl.
   Nothing needs to be uninstalled first.
3. The models and settings already on the laptop stay. The installer copies only the
   model files the laptop does not have yet (a file of the same size is skipped), so
   it is quick.
