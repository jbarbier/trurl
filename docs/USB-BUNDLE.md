# The USB package

Classrooms often have no internet, so Trurl is handed out on USB sticks: one
installer plus the models. Nothing has to be downloaded on the learners' computers.

## What is on the stick

```
Trurl-0.1.0-USB/
├── Trurl_0.1.0_x64-offline-setup.exe   ← learners double-click this
├── models/                             ← the AI models (GGUF files)
│   ├── gemma-3-1b-it-Q4_K_M.gguf      806 MB
│   ├── gemma-3-4b-it-Q4_K_M.gguf      2.5 GB
│   └── Qwen_Qwen3-0.6B-Q4_K_M.gguf    484 MB
├── linux/                              (only with --linux)
├── LICENSES/                           Gemma Terms of Use and every other license
├── READ-ME-FIRST.txt                   instructions for whoever holds the stick
└── SHA256SUMS.txt                      to check that a copy is complete
```

**Why a folder and not one big installer?** A Windows installer cannot be bigger than
2 GB, and the Gemma 3 4B model alone is 2.5 GB. The installer is also what Trurl
downloads when it updates itself, and nobody should download 3.8 GB of models for an
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
3. **About 7.6 GB of free disk:** 3.8 GB for the models (kept in a cache folder for
   next time) plus 3.8 GB for the finished folder.
4. **Internet for the first run:** about 3.9 GB to download, once.
5. **Sticks of 8 GB or more** for the full folder (3.8 GB). For 4 GB sticks, leave out
   Gemma 3 4B (see below). FAT32 and exFAT sticks both work, since no file is over 4 GB.

### Build it

```
python scripts/make_usb_bundle.py                # Windows
python3 scripts/make_usb_bundle.py               # Linux
python3 scripts/make_usb_bundle.py --linux       # also the Linux terminal version
```

- **The installer** is downloaded from GitHub Releases, for the version written in
  `pins.json` of your copy of the repository. That release must be published. To use
  an installer you already have, add `--installer path\to\Trurl_0.1.0_x64-offline-setup.exe`.
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

- **A smaller stick** with only the two small models (1.3 GB), for 4 GB sticks or
  4 GB laptops:
  ```
  python3 scripts/make_usb_bundle.py --only gemma-3-1b-it,qwen3-0.6b
  ```
- **One more model**, from any GGUF chat model file you have:
  ```
  python3 scripts/make_usb_bundle.py --extra-model path/to/model.gguf
  ```
  Or just copy the `.gguf` file into the stick's `models/` folder.
- **A model in every Trurl release** (pinned, checked by fingerprint, listed in the
  About screen with its license): add it to `models` in `pins.json` with its Hugging
  Face repository, commit, file name, size and SHA-256. Then add its license to
  `licenses/`.

## On the learner's computer

- **Where the models go:** `%LOCALAPPDATA%\CyberiadLabs\Trurl\models` on Windows.
- **Updates never touch them.** Uninstalling keeps them too, unless the learner ticks
  "Delete the application data" in the uninstaller.
- **Adding a model later** from a stick needs no reinstall: in Trurl, open
  **Models**, then **Import model**.
