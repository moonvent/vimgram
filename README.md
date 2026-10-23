# vimgram

Keyboard-only, Vim-style Telegram client: Rust + [iced](https://iced.rs) + TDLib.

- Vim keys in any keyboard layout, counts, `gg`/`G`, `:` commands, `/` search
- Folders, chat list and messages side by side; `Ctrl-h`/`Ctrl-l` between them
- Photos, albums, videos, round videos, GIFs, stickers (incl. animated), voice
  messages and music with speed control, recording, reactions, replies, forwards, edits,
  scheduled and silent messages, PDF export
- Deleted messages stay visible: everything seen is archived locally
- Notifications, tray icon and autostart (Linux)

Linux (Wayland, developed on Fedora + Hyprland) is the main platform; Windows
builds come from CI and are newer (see [Roadmap](#roadmap)).

## Telegram API credentials

Every Telegram client needs its own `api_id` and `api_hash`: create an app at
<https://my.telegram.org> → *API development tools*, then put them into
`config.toml` (path below for your system):

```toml
api_id = 123456
api_hash = "..."
theme = "Tokyo Night"   # optional, see :theme
folder = "Личка"        # optional: folder to start in (by name, any case)
tray = true             # optional: tray icon (waybar `tray` module)
close_to_tray = true    # optional: closing the window keeps vimgram in the tray
prefetch_mb = 50        # optional: auto-download media up to this size (see below)
open_on_select = true   # optional: moving through the chat list opens the chat beside it
                        # (read only once you move into its messages)
viewer = "side"         # optional: photos and videos open beside the chat, not full screen
clock = "12h"           # optional: times as 9:05 PM instead of 21:05
```

Never commit or share these: they identify your app to Telegram.

## Windows

Download `vimgram-windows-x86_64.zip` from
[Releases](../../releases) (or the latest CI run's artifacts), unpack it anywhere,
create `%APPDATA%\vimgram\config\config.toml` as above and run
`vimgram\bin\vimgram.exe`. TDLib, GStreamer and the emoji font are inside.

Building it yourself (what [CI](.github/workflows/ci.yml) does):

1. Rust (MSVC toolchain), Visual Studio Build Tools, LLVM (for bindgen), CMake.
2. GStreamer **MSVC** runtime + development installers from
   <https://gstreamer.freedesktop.org/download/>, `pkg-config`
   (`choco install pkgconfiglite`), and `PKG_CONFIG_PATH` set to
   `<gstreamer>\lib\pkgconfig`, `<gstreamer>\bin` in `PATH`.
3. `.\scripts\fetch-tdlib.ps1` and `.\scripts\fetch-emoji.ps1`.
4. `.\scripts\build-rlottie.ps1` (Lottie stickers), then add `vendor\rlottie`
   to `PKG_CONFIG_PATH`.
5. `cargo build --release`, then copy `vendor\tdlib\bin\*.dll` and
   `vendor\rlottie\build\Release\rlottie.dll` next to
   `target\release\vimgram.exe`.

## Linux (Fedora)

```sh
sudo dnf install libcxx libcxxabi llvm-libunwind   # runtime deps of prebuilt TDLib
sudo dnf install gstreamer1-devel gstreamer1-plugins-base-devel rlottie-devel  # media
sudo dnf install wl-clipboard ffmpeg               # pasting images/files; odd WebM stickers
./scripts/fetch-tdlib.sh                           # TDLib 1.8.61 -> vendor/tdlib
./scripts/fetch-emoji.sh                           # color emoji iced can draw
cargo run --release
```

iced can't draw COLRv1 color fonts, which is all Fedora ships for emoji, so
`fetch-emoji.sh` puts Noto Color Emoji (bitmap version) into
`~/.local/share/vimgram/fonts/`; without it emoji are blank.

## Keys

Normal mode works in any keyboard layout (`о` = `j`, `Shift+п` = `G`, `х` = `[`).

| Key | Action |
|---|---|
| `j` / `k`, `5j` | move down / up (with count) |
| `gg` / `G` | top / bottom |
| `Ctrl-d` / `Ctrl-u` | half page down / up |
| `h` / `l`, `Ctrl-h` / `Ctrl-l` | focus the pane to the left / right: folders ↔ chat list ↔ messages (`j`/`k`, `gg`/`G` move through folders there, `Enter` goes to the chats) |
| `Tab` | switch between chat list and messages |
| `J` / `K`, `gt` / `gT`, `3gt`, `Alt-1…9` | next / previous / n-th folder |
| `Enter`, `o` | open chat (a channel opens beside the list, focus stays there; `Enter` again goes to the posts); view photo or video; play voice message or music; show a spoiler (text or media; clicking it does too) |
| `i`, `I`, `a`, `A` | insert mode (write message / login field); `I` starts at the beginning of the message, `a`/`A` at its end |
| `r` | reply to the message under the cursor |
| `R` | react to the message: `h`/`j`/`k`/`l` + `Enter` or `1`–`9` picks, `/` searches, the same reaction again takes it back; clicking a reaction under a message toggles it too |
| `cc`, `e` | edit your message |
| `f` | forward: pick a chat, `Enter` sends; `v` marks several chats (✓), `Enter` then sends to all of them; `i` writes a comment that goes before the forwarded messages (`Enter` in it sends too) |
| `v` | mark / unmark the message (✓), with gaps between marked ones; then `f` forward, `y` copy their text, `d` delete all of them; `Esc` clears the marks |
| `V` | Visual: select messages with `j`/`k`/`gg`/`G`, then `f` forward, `y` copy their text, `d` delete, `v` marks the whole range; `V`/`Esc` ends it |
| `dd` | delete message: `y` for everyone, `m` only for me |
| `ga`, `:attach [path]` | attach photos, videos or files (system file picker without a path); `Enter` sends, the text is the caption |
| `gp`, click on the pinned bar, `:pin` | jump to the pinned message the bar shows (the newest one above the cursor; `3 / 5` on the right counts them, the oldest is 1); `gp` again goes to the one before it, round to the newest; after it `n` / `N` go to the older / newer pinned messages |
| `gP`, click on the `3 / 5` counter, `:pins` | only the pinned messages, oldest at the top, the cursor on the one the bar showed; `Enter` (on one without media to open) or `Esc` goes to the one under the cursor among all messages |
| `gf`, click on the reply quote or "Forwarded from" | go to the original of a forwarded message (channel posts and Saved Messages say where it is; otherwise its author's chat), or to the message replied to |
| `gx`, `2gx`, click on a link | open the (n-th) link of the message: t.me links and @mentions open here, at the post; web and e-mail links in the browser; a #hashtag shows only the chat's messages with it (`Esc` shows all again, at the message under the cursor) |
| `gu`, `gk` | the first unread message of the topmost private chat (`gu`) or channel (`gk`) with unread ones, opened in the folder it is in |
| `gc`, click on "💬 N comments" | the comments of a channel post: `i` comments, `r` answers a comment, `Esc` (or `Ctrl-o`) goes back to the post |
| `Ctrl-o`, `Ctrl-i` | back to the chat (and message, and folder) you came from — by `gu`/`gk`, `gf`, `gc`, a t.me link or opening another chat — and forward again |
| `ge`, `Ctrl-.` in insert mode, `:emoji`, the 😀 button | emoji panel: `h`/`j`/`k`/`l` move, `Enter` inserts, `Tab`/`[`/`]`/`1`–`0` groups, `/` search by name, `Esc` closes |
| `yy`, `Y` | copy message text (or chat title) to the clipboard |
| `p` / `P` | paste the clipboard at the end / start of the message; a copied image or file becomes an attachment |
| `q` | record a voice message; `q`/`Enter` sends, `Esc` discards |
| `Space` | play / pause voice message, music or video |
| `gv`, click on the player bar | back to the voice message or music playing in another chat (the bar above the chat shows it; ✕ stops) |
| `]` `+` / `[` `-` / `=` | speed up / down (0.25x–5x) / reset to 1x; voice messages, music, videos and round videos each remember their own speed |
| `m` | mute or unmute the chat (under the cursor in the list, otherwise the open one) |
| `U` | leave the group or unsubscribe from the channel (asks first); joins one you are not in. Channels you can't post in show these two keys instead of the compose line |
| `}` / `{`, `:volume 70` | volume up / down by 10% (or set it); one volume for videos, voice messages and music, remembered |
| `H` / `L` | seek voice message or music −5s / +5s; with the mouse: click on a voice message's waveform, drag the progress of music or of a video |
| `Esc`, `Ctrl-[` | normal mode; cancel reply, edit, forward, filter, attachment; with nothing to cancel asks to quit (`y`/`Enter`) |
| `/` | chat list: filter chats, and search Telegram: your other chats, public channels, groups and users, then the messages of all your chats below them (`Enter` on one opens its chat there); message pane: search messages (`/#tag`: only the messages with that hashtag, like clicking it) |
| `n` / `N` | next (older) / previous (newer) search match, or pinned message after `gp` |
| `G`, the ↓ button | back to the latest messages (also after a search or date jump) |
| `:q` | quit |
| `:date 01.05.2024` (`01.05`, `2024-05-01`, `yesterday`) | jump to a day |
| `:silent`, `Alt-Enter` in insert mode | send the composed message without sound |
| `:later 21:00` (`+30m`, `+2h`, `01.11 09:00`) | schedule the composed message |
| `:export [path]` | export the chat to PDF (default `~/Documents/vimgram/`) |
| `:posts [query]` | also find posts of public channels for the chat filter (or `query`), below the other results; Telegram allows a few free searches a day, none paid |
| `:search word`, `:folder name`, `:theme name`, `:speed 1.5`, `:42` | commands |
| `:autostart [on\|off]` | start with the session, in the tray (off by default) |
| `:cleancache [days]` | delete cached media unused for that many days (default 7; also done weekly) |
| `:help` | key summary |

Command line: `↑`/`↓` (or `Ctrl-p`/`Ctrl-n`) recall earlier `/` searches and `:` commands
starting with what you typed; the history survives restarts.

Insert mode: `Enter` sends, `Shift+Enter` new line, `Ctrl-v` paste (text, or a copied
image / files as attachments), `Ctrl-w` delete word, `Ctrl-u` delete to start, `↑` in an empty line edits your last
message. `Esc` with some text written goes to Text mode instead of the message list.

Text mode (`TEXT` in the status line): Vim keys inside the message being written. `h`/`l`,
`w`/`b`/`e` (`W`/`B`/`E`), `0`/`^`/`$`, `j`/`k` between lines, `gg`/`G` move; `d`, `c`, `y`
take a motion (`dw`, `cw`, `d$`, `y2w`), doubled for the whole line (`dd`, `cc`, `yy`);
`x`/`X`, `D`, `C`, `s`/`S`, `Y`, `~`; `i`/`a`/`I`/`A`/`o`/`O` go back to typing; `p`/`P`
paste; `u` undo, `Ctrl-r` redo; counts work (`3w`, `2dd`). `Enter` sends, `Esc` goes to the
message list, the text stays. Two to ten photos and videos go as an album. Music files (mp3, m4a, flac, ogg, opus,
wav) go as music, with title and artist from their tags; several of them as an album too.

When a voice or round video message ends and the very next message is one too, it plays
next (voice messages also from another chat; round videos while their chat is open). Music
plays on the same way, track after track, but only into more music. A voice message or track left
for another one (or for a video, or stopped with ✕) goes on where it was, 5 seconds earlier.

Photo/video viewer: `j`/`k` next/previous, `h`/`l` seek ±5s, `H`/`L` ±30s, `Space` pause,
`[` `]` `=` speed, `↑`/`↓` (or `{` `}`) volume, `m` sound off / on, `Esc` close. Under a video: time played, progress and length.
`s` switches between full screen and beside the chat (`viewer = "side"` starts there). Beside the
chat `Tab` (or `Ctrl-h`) gives the keys back to the messages while it plays on, `Tab` or `l` there
(or a click on it) returns to it, and `Esc` in the messages closes it.

Mouse: click folders, chats, messages and media; the wheel moves the cursor.

A message taller than the window scrolls with `j`/`k` before the cursor moves on.

## Archive, notifications, tray

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

Tray (Linux): left click shows / hides the window, the menu has Quit. Wayland can't hide a
window, so "hide" closes it and vimgram keeps running; `:q` quits. `:autostart on`
enables a systemd user service bound to `graphical-session.target` when the session has
one (uwsm, or a Hyprland config starting such a target), otherwise an XDG autostart
entry; both start vimgram with `--hidden`, in the tray.

App menu (Linux): every start of a release build writes
`~/.local/share/applications/vimgram.desktop` pointing at that executable, so
launchers (`wofi --show drun`, rofi, fuzzel) and desktops list vimgram after the first
`cargo run --release`.

## Where things are

| Linux | Windows | What |
|---|---|---|
| `~/.config/vimgram/config.toml` | `%APPDATA%\vimgram\config\config.toml` | settings |
| `~/.local/share/vimgram/db` | `%APPDATA%\vimgram\data\db` | TDLib database (session, chats, sticker files) |
| `~/.local/share/vimgram/files` | `%APPDATA%\vimgram\data\files` | media cache: photos, videos, voice messages, documents. Files unused for a week are deleted weekly (`:cleancache`) |
| `~/.local/share/vimgram/archive.sqlite`, `media/` | `%APPDATA%\vimgram\data\…` | archive of messages and their media (kept) |
| `~/.cache/vimgram` | `%LOCALAPPDATA%\vimgram\cache` | pasted images, notification images |

Nothing is saved to `~/Downloads`. The session in `db` is a logged-in Telegram
account: keep it private.

## Development

`[profile.release]` builds incrementally: after a change `cargo run --release` rebuilds in
seconds; without changes it starts right away. `./target/release/vimgram` runs the last
build without checking for changes.

## Roadmap

- Windows: tray icon, notifications under vimgram's own name that are withdrawn
  once read, an installer with a Start menu entry, a slimmer GStreamer bundle
- Linux release builds (TDLib next to the binary, `$ORIGIN` rpath)
- Custom emoji in reactions and messages

## Releases

Pushes to `main` and pull requests are built on Linux, Windows and macOS and
tested on Linux and macOS. A push to `main` that raises `version` in
`Cargo.toml` publishes a release once the build passes: tag `vX.Y.Z` and the
Windows zip on [Releases](../../releases). Pushing a `vX.Y.Z` tag by hand
does the same.

## License

[MIT](LICENSE). Release builds bundle TDLib (BSL-1.0), GStreamer (LGPL-2.1+,
linked dynamically), rlottie (MIT) and Noto Color Emoji (OFL-1.1) under their own licenses.
