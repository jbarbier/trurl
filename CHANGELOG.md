# Changelog

Each section is also the release notes the update prompt shows, so write it
for learners and teachers: short, plain, what changes for them.

## 0.1.2

- New versions show up more reliably. If Trurl cannot reach GitHub when it looks
  for an update, it tries again after 30 minutes instead of the next day.
- About says when Trurl last looked for an update and what it found. If it could
  not check, About says so, and Details shows the reason.
- When a new version is out, About has an Update now button. You can also update
  after clicking Later on the card above the chat.

## 0.1.1

- Chats: every conversation is saved on your computer, and only there. Open an old
  chat to carry on where you stopped, with the same settings. Delete one chat, or
  all of them.
- The course models are one click away: the Hugging Face screen starts with For the
  course (Gemma 3 1B, Gemma 3 1B PT, Qwen3 0.6B), the exact files the course uses.
  No searching, no look-alikes.
- A download you start scrolls into view, so you see its progress bar.
- Models you downloaded no longer say "Included with Trurl": course models say
  "Course model", other checked files "Checked by Trurl".
- About has a new switch, Check automatically. Turn it off and Trurl looks for
  updates only when you click Check for updates.

## 0.1.0

First version of Trurl.

- Chat with open-weight models on your own computer, without internet.
- See the model think: reasoning shows apart from the answer, with how long it took.
- Change the system prompt, the temperature and the context size.
- Import models from a USB stick, delete them, or download new ones from Hugging Face.
- Downloads continue where they stopped when the connection drops.
- Gemma 3 1B, Gemma 3 4B and Qwen3 0.6B come on the USB stick.
