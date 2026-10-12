TRURL for Linux, by Cyberiad Labs
=================================

Trurl runs small open-weight language models on your own computer, without
internet, so you can see how they work.

Install (no administrator password needed):

    sh install.sh

Then start it with:

    trurl

If "trurl" is not found, log out and back in once (so ~/.local/bin is on your
PATH), or start it with:

    ~/.local/bin/trurl

If a "models" folder is on the same USB stick (next to this folder, or two
folders up), the installer copies those models to ~/.local/share/trurl/models.

Keys inside Trurl:
    Enter              send your message
    Alt+Enter, Ctrl+J  new line (Alt+Enter may be taken by your terminal)
    Esc                stop the answer
    Ctrl+N             new chat
    Ctrl+T             show or hide the model's thinking
    F2                 models (use, import, delete)
    F3                 settings (system prompt, temperature, context size)
    F4                 download models from Hugging Face (needs internet)
    F1                 about, licenses, check for updates
    F7 / F8            update now / later, when an update is offered
    F12                Hugging Face access token (only for gated models)
    Ctrl+Q             quit

Works on: Ubuntu 20.04 or newer, Debian 11 or newer, Linux Mint 20 or newer,
and other 64-bit Linux with glibc 2.31 or newer. Any 64-bit Intel or AMD CPU.

Uninstall (asks before deleting your models):

    sh ~/.local/share/trurl/app/install.sh --remove

More: https://github.com/jbarbier/trurl
