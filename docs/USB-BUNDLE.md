# The USB package

Classrooms often have no internet, so Trurl is handed out on USB sticks: one
installer plus the models, nothing to download on the learners' computers.

## Why a folder and not one big installer

A single Windows installer (NSIS) cannot hold more than 2 GB, and Gemma 3 4B alone
is 2.5 GB. The same installer is also what the auto-updater downloads, and learners
should not download 3.8 GB of models for every update.

So the stick holds a folder:

```
Trurl-0.1.0-USB/
├── Trurl_0.1.0_x64-offline-setup.exe   ← learners double-click this
├── models/
│   ├── gemma-3-1b-it-Q4_K_M.gguf      806 MB
│   ├── gemma-3-4b-it-Q4_K_M.gguf      2.5 GB
│   └── Qwen_Qwen3-0.6B-Q4_K_M.gguf    484 MB
├── LICENSES/                           Gemma Terms of Use and every other license
├── READ-ME-FIRST.txt
└── SHA256SUMS.txt
```

The installer copies every `.gguf` it finds in the `models` folder next to it into
the learner's models folder:

- **No partial models.** Each file is copied as `.part` and renamed only once it
  is complete, so pulling the stick out midway never leaves a damaged model that
  looks whole.
- **No repeat copies.** Files already on the computer with the same size are
  skipped, so a second install is quick.
- **A check on first launch.** Trurl compares each bundled model against its
  SHA-256 and marks a damaged copy as such.

The `offline-setup` installer also contains the Microsoft WebView2 runtime
installer, for Windows 10 computers that do not have WebView2 yet. Windows 11
always has it.

## Make the folder

```
python3 scripts/make_usb_bundle.py               # downloads the latest release installer
python3 scripts/make_usb_bundle.py --linux       # also the Linux terminal version
```

- The models are downloaded once into `~/.cache/trurl/models` (resumable,
  hash-checked) and copied from there.
- The result is `dist/Trurl-<version>-USB/` (3.8 GB).
- Copy the whole folder onto each stick. FAT32 and exFAT both work, because the
  largest file is under 4 GB.
- To check a copied stick: `sha256sum -c SHA256SUMS.txt` (Linux) or compare
  `Get-FileHash` output (Windows).

## Add more models to the stick

Any GGUF chat model works. Either:

- put the `.gguf` file into the stick's `models/` folder, or
- pass it when building: `python3 scripts/make_usb_bundle.py --extra-model path/to/model.gguf`.

A smaller stick with only the two small models (1.3 GB):

```
python3 scripts/make_usb_bundle.py --only gemma-3-1b-it,qwen3-0.6b
```

To make a model part of every Trurl release (pinned, hash-checked, listed in the
About screen with its license), add it to `models` in `pins.json` with its
Hugging Face repo, commit SHA, file name, size and SHA-256, and add its license
to `licenses/`.

## On the learner's computer

- Models go to `%LOCALAPPDATA%\CyberiadLabs\Trurl\models`.
- Updates and uninstalls never touch them. The uninstaller asks whether to delete
  them, and the default is No.
- A learner can also add a model later from the stick, without reinstalling, from
  Trurl's Models screen with **Import model**.
