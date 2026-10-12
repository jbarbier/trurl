TRURL for Linux, by Cyberiad Labs
=================================

Trurl runs small open-weight language models on your own computer, without
internet, so you can see how they work.

Install (no administrator password needed):

    sh install.sh

Then start it with:

    trurl

If a "models" folder is next to this file (the USB stick package), the
installer copies those models to ~/.local/share/trurl/models.

Keys inside Trurl:
    Enter        send your message
    Alt+Enter    new line
    Esc          stop the answer
    Ctrl+N       new chat
    Ctrl+T       show or hide the model's thinking
    F2           models (use, import, delete)
    F3           settings (system prompt, temperature, context size)
    F4           download models from Hugging Face (needs internet)
    F1           about, licenses, check for updates
    Ctrl+Q       quit

Works on: Ubuntu 20.04 or newer, Debian 11 or newer, Linux Mint 20 or newer,
and other 64-bit Linux with glibc 2.31 or newer. Any 64-bit Intel or AMD CPU.

Uninstall (asks before deleting your models):

    sh ~/.local/share/trurl/app/install.sh --remove

More: https://github.com/jbarbier/trurl
