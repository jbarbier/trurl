# Core ↔ front end contract

Version 1. Source of truth: `crates/trurl-core/src/events.rs` (events),
`crates/trurl-core/src/api.rs` (commands) and, for what only the desktop app has
(`update_*` events, `update_check`, `update_status`, `update_install`, `open_models_folder`),
`apps/desktop/src-tauri/src/main.rs`. Both front ends (desktop UI over Tauri or the
dev bridge, terminal app in-process) use exactly this.

## Commands

All commands are asynchronous. A command that fails rejects with a `UserError`
(see below) and changes nothing.

| Command | Arguments | Returns |
|---|---|---|
| `snapshot` | | `Snapshot` |
| `send` | `{ text }` | turn id (number) |
| `stop` | | |
| `new_chat` | | |
| `open_chat` | `{ id }` | (the chat's turns and the settings of its last answer come back; a `snapshot` follows) |
| `delete_chat` | `{ id }` | (deleting the chat on screen also clears it: `chat_cleared`) |
| `delete_all_chats` | | (every saved chat, and the chat on screen) |
| `set_system_prompt` | `{ text }` | |
| `set_temperature` | `{ value }` (0.0 to 2.0, step 0.1) | |
| `set_context` | `{ value }` (one of `settings.context_choices`) | |
| `select_model` | `{ file }` | |
| `reload` | | |
| `import_model` | `{ path }` | |
| `delete_model` | `{ file }` | |
| `hub_online` | | boolean |
| `hub_search` | `{ query }` | `RepoHit[]` |
| `hub_details` | `{ repo }` | `RepoDetails` |
| `download_start` | `{ repo, file, revision, size, sha256 }` | download id |
| `download_course_model` | `{ id }` (a `course_models` id) | download id. The core uses the pinned repo, commit, file, size and SHA-256; nothing comes from the UI |
| `download_pause` / `download_resume` / `download_cancel` | `{ id }` | |
| `set_hf_token` | `{ token }` | |
| `set_auto_update` | `{ on }` | (off: no update check unless the user asks; saved as `settings.auto_update_check`) |
| `licenses` | | `{ name, text }[]` |
| `open_models_folder` | | |
| `update_check` | | `UpdateInfo` or `null` |
| `update_status` | | the last automatic check, or `null` if none ran since start: `{ at` (seconds since 1970)`, outcome: "running" \| "newer" \| "latest" \| "failed", update?` (`UpdateInfo`, when newer)`, error?` (a `UserError`, when failed)`, retry_in_mins? }` (`trurl_core::update::check_status`). The UI reads it at start, every minute and in About, so an `update_available` it missed still gets offered |
| `update_install` | | (the app restarts when done) |

The UI also gets `pickModelFile()` from `api.js`, which opens the OS file picker
filtered to `.gguf` and resolves to a path or `null`.

## Events

Every event is a JSON object with a `type` field.

| `type` | Fields | Meaning |
|---|---|---|
| `snapshot` | `snapshot` | Full state. Sent on start and after reconnecting. |
| `engine` | `engine` | The model server changed state (see `EngineState`). |
| `models` | `models` | The models folder changed. |
| `settings` | `settings` | Saved settings changed. |
| `generating` | `generating` | A reply started or ended. Disable Send, show Stop. |
| `turn_started` | `turn` | A new question/answer pair, status `streaming`. |
| `turn_delta` | `id`, optional `reasoning`, optional `answer`, optional `reasoning_from_answer` | Append text. If `reasoning_from_answer` is true, move the answer text so far into the reasoning block first. |
| `turn_thought` | `id`, `ms` | The first answer token came after `ms` of thinking. Collapse the thinking block and label it "Thought for Ns". |
| `turn_finished` | `turn` | Final state of the turn (`done`, `stopped`, `cut`, `error`). |
| `chat_cleared` | | New chat. |
| `chats` | `chats`, `chat_id` | The saved chats or the chat on screen changed (saved after an answer, deleted, opened, new chat). A chat that was only looked at is not saved again. |
| `import_progress` | `file`, `done`, `total` | Copying a model in. |
| `import_finished` | `file` or `error` | |
| `verify_progress` | `file`, `done`, `total` | Checking a bundled model's SHA-256. |
| `downloads` | `downloads` | Full list of downloads. |
| `notice` | `error` | A failure outside a turn: show it as a dismissible banner. |
| `update_available` | `update` | Desktop only. `{ version, notes, date }`. |
| `update_progress` | `downloaded`, `total` | Desktop only. |
| `update_failed` | `error` | Desktop only. |

## Shapes

```
UserError   { code, title, hint, detail? }
EngineState { state: "idle" }
            | { state: "loading", file, name }
            | { state: "ready", file, name, context }
            | { state: "failed", file, name, error }
ModelInfo   { file, name, quant?, size, fit: "green"|"orange"|"red", bundled, course, verified?,
              broken, has_chat_template, reasoning_hint, train_context? }
              bundled: name and size match a pinned model (pins.json); course: also a course model
Turn        { id, user, reasoning, answer, model, status, thought_ms?, error?,
              system_prompt, temperature, context }   (the settings it was written with)
              status: "streaming" | "done" | "stopped" | "cut" | "error"
Settings    { system_prompt, temperature, context_size, model?, has_hf_token,
              default_temperature, default_context, context_choices[] }
Snapshot    { version, llama_build, models_dir, hardware{ total_ram, physical_cores, threads,
              cpu_name, variant }, settings, models[], engine, turns[], generating, downloads[],
              course_models[], chats[], chat_id? }
              chat_id: the chat on screen in the history; null until its first answer is saved
              variant: "avx2"|"avx"|"ssse3"|"compat", the llama-server build for this processor
CourseModel { id, name, note, file, size, fit, download_id }   the course models in course order, fixed
              per version. download_id: the id of its pinned download; match downloads on it, not on file
ChatSummary { id, title, updated, turns, model }   newest first; updated: seconds since 1970;
              title: the first question, one line, at most 60 characters
RepoHit     { id, downloads, likes, gated, updated?, chat? }
              chat: false = this model does not chat (embeddings, speech, rerankers)
RepoDetails { id, author?, revision, gated, license?, downloads, likes, updated?, base_model?,
              architecture?, context_length?, parameters?, tags[], chat?, readme?, files: RepoFile[] }
RepoFile    { path, size, sha256?, quant?, fit, split, recommended }
              recommended: the one file to pick on this computer, listed first (Q4_K_M with
              AVX2; Q4_0 when there is one on an older processor; see hub::recommend)
Download    { id, repo, file, revision, dest_file, size, sha256?, done, rate, user_paused, synced,
              status: "running"|"retrying"|"paused"|"verifying"|"failed"|"done", error? }
              synced: bytes known to be on disk (resume point after a power cut)
UpdateInfo  { version, notes, date? }
```
