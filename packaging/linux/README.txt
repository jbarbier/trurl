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
    Enter              send your message (while a model loads, it is sent when ready)
    Alt+Enter, Shift+Enter, Ctrl+J
                       new line (your terminal may take Alt+Enter or Shift+Enter)
    Ctrl+U             clear the message box
    Esc                stop the answer
    Ctrl+N             new chat
    Ctrl+T             show or hide the model's thinking (every answer)
    F6                 copy the last answer; it is also added, with the question
                       and settings, to "Trurl notes.txt" in your Documents folder
    F5                 after "ran out of room": the next context size, same question
    Ctrl+R             load the model again (after an error)
    PgUp / PgDn        scroll the conversation
    F2                 models (use, import, delete)
    F3                 settings (system prompt, temperature, context size)
    F4                 download models from Hugging Face (needs internet)
    F1                 about, licenses, check for updates
    F7 / F8            see what is new and update / later, when an update is offered
    F12                Hugging Face access token (only for gated models)
    Ctrl+Q             quit

Works on: Ubuntu 20.04 or newer, Debian 11 or newer, Linux Mint 20 or newer,
and other 64-bit Linux with glibc 2.31 or newer. Any 64-bit Intel or AMD CPU.

Uninstall (asks before deleting your models, settings and Hugging Face token):

    sh ~/.local/share/trurl/app/install.sh --remove

More: https://github.com/jbarbier/trurl
