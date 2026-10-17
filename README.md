# vimgram

Keyboard-only, Vim-style Telegram client: Rust + [iced](https://iced.rs) + TDLib.

## Setup (Fedora)

```sh
sudo dnf install libcxx libcxxabi llvm-libunwind   # runtime deps of prebuilt TDLib
sudo dnf install gstreamer1-devel gstreamer1-plugins-base-devel rlottie-devel  # media
sudo dnf install wl-clipboard ffmpeg               # pasting images/files; odd WebM stickers
./scripts/fetch-tdlib.sh                           # TDLib 1.8.61 -> vendor/tdlib
./scripts/fetch-emoji.sh                           # color emoji iced can draw
```

iced can't draw COLRv1 color fonts, which is all Fedora ships for emoji, so
`fetch-emoji.sh` puts Noto Color Emoji (bitmap version) into
`~/.local/share/vimgram/fonts/`; without it emoji are blank.

Create `~/.config/vimgram/config.toml` with credentials from https://my.telegram.org:

```toml
api_id = 123456
api_hash = "..."
theme = "Tokyo Night"   # optional, see :theme
folder = "Личка"        # optional: folder to start in (by name, any case)
tray = true             # optional: tray icon (waybar `tray` module)
close_to_tray = true    # optional: closing the window keeps vimgram in the tray
prefetch_mb = 50        # optional: auto-download media up to this size (see below)
```

```sh
cargo run --release
```

## Keys

Normal mode works in any keyboard layout (`о` = `j`, `Shift+п` = `G`, `х` = `[`).

| Key | Action |
|---|---|
| `j` / `k`, `5j` | move down / up (with count) |
| `gg` / `G` | top / bottom |
| `Ctrl-d` / `Ctrl-u` | half page down / up |
| `h` / `l` | focus the pane to the left / right: folders ↔ chat list ↔ messages (`j`/`k`, `gg`/`G` move through folders there, `Enter` goes to the chats) |
| `Tab` | switch between chat list and messages |
| `J` / `K`, `gt` / `gT`, `3gt`, `Alt-1…9` | next / previous / n-th folder |
| `Enter`, `o` | open chat (a channel opens beside the list, focus stays there; `Enter` again goes to the posts); view photo or video; play voice message |
| `i`, `a` | insert mode (write message / login field) |
| `r` | reply to the message under the cursor |
| `cc`, `e` | edit your message |
| `f` | forward: pick a chat, `Enter` sends |
| `dd` | delete message: `y` for everyone, `m` only for me |
| `ga`, `:attach [path]` | attach photos, videos or files (system file picker without a path); `Enter` sends, the text is the caption |
| `gp` | jump to the pinned message |
| `ge`, `Ctrl-.` in insert mode, `:emoji`, the 😀 button | emoji panel: `h`/`j`/`k`/`l` move, `Enter` inserts, `Tab`/`[`/`]`/`1`–`0` groups, `/` search by name, `Esc` closes |
| `yy`, `Y` | copy message text (or chat title) to the clipboard |
| `p` / `P` | paste the clipboard at the end / start of the message; a copied image or file becomes an attachment |
| `q` | record a voice message; `q`/`Enter` sends, `Esc` discards |
| `Space` | play / pause voice message or video |
| `gv`, click on the player bar | back to the voice message playing in another chat (the bar above the chat shows it; ✕ stops) |
| `]` `+` / `[` `-` / `=` | speed up / down (0.25x–5x) / reset to 1x; voice messages, videos and round videos each remember their own speed |
| `H` / `L` | seek voice message −5s / +5s |
| `Esc`, `Ctrl-[` | normal mode; cancel reply, edit, forward, filter, attachment |
| `/` | chat list: filter chats; message pane: search messages |
| `n` / `N` | next (older) / previous (newer) search match |
| `G` | after a search or date jump: back to the latest messages |
| `:q` | quit |
| `:date 01.05.2024` (`01.05`, `2024-05-01`, `yesterday`) | jump to a day |
| `:silent`, `Alt-Enter` in insert mode | send the composed message without sound |
| `:later 21:00` (`+30m`, `+2h`, `01.11 09:00`) | schedule the composed message |
| `:export [path]` | export the chat to PDF (default `~/Documents/vimgram/`) |
| `:search word`, `:folder name`, `:theme name`, `:speed 1.5`, `:42` | commands |
| `:autostart [on\|off]` | start with the session, in the tray (off by default) |
| `:cleancache [days]` | delete cached media unused for that many days (default 7; also done weekly) |
| `:help` | key summary |

Command line: `↑`/`↓` (or `Ctrl-p`/`Ctrl-n`) recall earlier `/` searches and `:` commands
starting with what you typed; the history survives restarts.

Insert mode: `Enter` sends, `Shift+Enter` new line, `Ctrl-v` paste (text, or a copied
image / files as attachments), `Ctrl-w` delete word, `Ctrl-u` delete to start, `↑` in an empty line edits your last
message. Two to
ten photos and videos go as an album.

When a voice or round video message ends and the very next message is one too, it plays
next (voice messages also from another chat; round videos while their chat is open).

Photo/video viewer: `j`/`k` next/previous, `h`/`l` seek ±5s, `H`/`L` ±30s, `Space` pause,
`[` `]` `=` speed, `Esc` close.

Mouse: click folders, chats, messages and media; the wheel moves the cursor.

A message taller than the window scrolls with `j`/`k` before the cursor moves on.

Every message the client sees is kept whole in `~/.local/share/vimgram/archive.sqlite`,
and every media file it downloads is hard-linked into `~/.local/share/vimgram/media/`
(no extra space while TDLib has the file too). Messages deleted later stay visible with
their photos, voice messages, stickers and videos, marked as deleted. TDLib removes the
files of deleted messages, so media of incoming messages in private chats and groups is
downloaded right away (up to `prefetch_mb`). Links of messages still alive after 30 days
are dropped.

New messages raise desktop notifications with the chat photo (and the app icon in its
corner) unless the chat is muted or open and focused; clicking one opens the chat. Once
the chat is read (here or on another device) its notifications are withdrawn.

Tray: left click shows / hides the window, the menu has Quit. Wayland can't hide a
window, so "hide" closes it and vimgram keeps running; `:q` quits. `:autostart on`
enables a systemd user service bound to `graphical-session.target` when the session has
one (uwsm, or a Hyprland config starting such a target), otherwise an XDG autostart
entry; both start vimgram with `--hidden`, in the tray.

Where things are:

| Path | What |
|---|---|
| `~/.local/share/vimgram/db` | TDLib database (session, chats, sticker files) |
| `~/.local/share/vimgram/files` | media cache: photos, videos, voice messages, documents. Files unused for a week are deleted weekly (`:cleancache`) |
| `~/.local/share/vimgram/archive.sqlite`, `media/` | archive of messages and their media (kept) |
| `~/.cache/vimgram` | pasted images, notification images |

Nothing is saved to `~/Downloads`.

`[profile.release]` builds incrementally: after a change `cargo run --release` rebuilds in
seconds; without changes it starts right away. `./target/release/vimgram` runs the last
build without checking for changes.
