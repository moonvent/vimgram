# vimgram

Keyboard-only, Vim-style Telegram client: Rust + [iced](https://iced.rs) + TDLib.

## Setup (Fedora)

```sh
sudo dnf install libcxx libcxxabi llvm-libunwind   # runtime deps of prebuilt TDLib
sudo dnf install gstreamer1-devel gstreamer1-plugins-base-devel rlottie-devel  # media
./scripts/fetch-tdlib.sh                           # TDLib 1.8.61 -> vendor/tdlib
```

Create `~/.config/vimgram/config.toml` with credentials from https://my.telegram.org:

```toml
api_id = 123456
api_hash = "..."
theme = "Tokyo Night"   # optional, see :theme
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
| `h` / `l`, `Tab` | focus chat list / messages |
| `J` / `K`, `gt` / `gT`, `3gt`, `Alt-1…9` | next / previous / n-th folder |
| `Enter`, `o` | open chat; view photo or video; play voice message |
| `i`, `a` | insert mode (write message / login field) |
| `r` | reply to the message under the cursor |
| `cc`, `e` | edit your message |
| `f` | forward: pick a chat, `Enter` sends |
| `dd` | delete message: `y` for everyone, `m` only for me |
| `ga`, `:attach [path]` | attach photos, videos or files (system file picker without a path) |
| `gp` | jump to the pinned message |
| `yy`, `Y` | copy message text (or chat title) to the clipboard |
| `p` / `P` | paste the clipboard at the end / start of the message |
| `q` | record a voice message; `q`/`Enter` sends, `Esc` discards |
| `Space` | play / pause voice message or video |
| `]` `+` / `[` `-` / `=` | speed up / down (0.25x–5x) / reset to 1x |
| `H` / `L` | seek voice message −5s / +5s |
| `Esc`, `Ctrl-[` | normal mode; cancel reply, edit, forward, filter |
| `/` | chat list: filter chats; message pane: search messages |
| `n` / `N` | next (older) / previous (newer) search match |
| `G` | after a search or date jump: back to the latest messages |
| `:q` | quit |
| `:date 01.05.2024` (`01.05`, `2024-05-01`, `yesterday`) | jump to a day |
| `:silent`, `Alt-Enter` in insert mode | send the composed message without sound |
| `:later 21:00` (`+30m`, `+2h`, `01.11 09:00`) | schedule the composed message |
| `:export [path]` | export the chat to PDF (default `~/Documents/vimgram/`) |
| `:search word`, `:folder name`, `:theme name`, `:speed 1.5`, `:42` | commands |
| `:help` | key summary |

Command line: `↑`/`↓` (or `Ctrl-p`/`Ctrl-n`) recall earlier `/` searches and `:` commands
starting with what you typed; the history survives restarts.

Insert mode: `Enter` sends, `Shift+Enter` new line, `Ctrl-v` paste, `Ctrl-w` delete word,
`Ctrl-u` delete to start.

Photo/video viewer: `j`/`k` next/previous, `h`/`l` seek ±5s, `H`/`L` ±30s, `Space` pause,
`[` `]` `=` speed, `Esc` close.

Mouse: click folders, chats, messages and media; the wheel moves the cursor.

A message taller than the window scrolls with `j`/`k` before the cursor moves on.

Every message the client sees is kept in `~/.local/share/vimgram/archive.sqlite`;
messages deleted later stay visible, marked as deleted. New messages raise desktop
notifications unless the chat is muted or open and focused.

Session data lives in `~/.local/share/vimgram`.
