# Control protocol

A running **Trigon Studio** listens for an agent's requests on `127.0.0.1` only, as JSON lines.
Every request runs on the window's thread, through the one session the owner works in, so the
owner watches each command land: in the activity feed (by the agent's name, with its parameters,
result and duration) and in the model, where what it changed is highlighted for a few seconds.
`trigon mcp` bridges an MCP client to this channel ([below](#mcp)); most agents never speak it
directly.

The design follows Polykrom's control channel (`~/wckdboy/polykrom/docs/control-protocol.md`), so
both apps behave alike: the same token handshake, limits, timeouts and `control.pending`. The code
is in [`crates/control`](../crates/control).

## Where it listens

| Launch | What listens |
|---|---|
| `trigon-studio --control PORT`, or `TRIGON_CONTROL_PORT=PORT` | exactly `127.0.0.1:PORT` (the flag wins) |
| neither | the default port **27900**, or the next free one up to 27915 (a second window, another program) |
| `trigon-studio --no-control` | nothing |

Trigon's ports sit clear of Polykrom's (27878–27893), so both apps can run at once.

### Finding the port

Studio writes its port to a discovery file beside the token files, `control/current.port` (same
folder and permissions as below), and removes it at exit unless a later window has replaced it.
It holds only the port, never the token. `trigon mcp` with no flags reads it, checks that
something listens there and bridges to that window; with no Studio running (or `--headless`, or
`--doc FILE`) it runs a session of its own. With several windows open, the one started last is
found; `--connect 127.0.0.1:PORT` picks another.

## Token

Every connection must authenticate. At start Studio makes a random 256-bit token for this launch
(from the operating system's random source) and writes it, as one line of hex, to a file only the
user can read:

| System | Token file |
|---|---|
| macOS | `~/Library/Application Support/Trigon/control/PORT.token` |
| Windows | `%LOCALAPPDATA%\Trigon\control\PORT.token` |
| Linux | `$XDG_DATA_HOME/trigon/control/PORT.token` (`~/.local/share/trigon/control/PORT.token`) |

On Unix the folder is `0700` and the file `0600`; on Windows the file's inherited permissions are
replaced by a single entry for the user. `TRIGON_CONTROL_DIR` moves the folder. The file is
removed when Studio exits, and the next launch writes a new token. The path is printed to stderr
at start.

The first request on a connection must carry the token as a top-level `"token"` member; later
requests on the same connection may leave it out. A first line without the right token (or one
that isn't JSON) is answered with `{"ok": false, "error": "unauthorized: …"}` and the connection
is closed without running anything. Tokens are compared in constant time.

`trigon mcp` reads the token file for the port by itself; `--token T` or `TRIGON_CONTROL_TOKEN`
give one explicitly. For a test harness, or a sandbox whose data folder the client can't see,
start Studio with `TRIGON_CONTROL_TOKEN` set to at least 32 hex characters and it uses that token
instead of making one (the file is still written).

## Requests and replies

Each request is one line `{"id": …, "method": "…", "params": {…}, "token": "…"}`; each reply is
one line `{"id": …, "ok": true, "result": …}` or `{"id": …, "ok": false, "error": "…"}`. The `id`
comes back as sent (any JSON value). Replies come in order, one per request.

### Limits

The channel is untrusted input; nothing a client sends can crash Studio.

- A request line may be at most **16 MiB**; a longer one is answered `request too large` and the
  connection is closed.
- A connection that sends **8** lines that are not JSON is closed.
- At most **16** connections are served at once; further ones get `too many connections`.
- At most **64** requests wait for the window at once (more are answered `busy`), and the window
  takes at most 64 per frame, so a flood can't starve drawing.
- A request the window hasn't answered within **60 s** is answered
  `{"ok": false, "running": true, "error": "timeout: …"}`. It has not failed: it still runs, and a
  change still applies when it finishes, so don't send it again. `control.pending` returns its
  reply once it has one (`{"pending": id, "method": …, "done": false}` while it runs, then
  `{"pending": id, "method": …, "done": true, "reply": {…}}` once). Until then, any other request
  on that connection is refused at once with `{"ok": false, "busy": true, …}` and not run.

## Methods

| Method | Params | Result |
|---|---|---|
| `control.hello` | `client`: a name (at most 64 characters) | `{app, version, client, connection, methods}`. The name labels this connection's commands in the activity feed and the agent indicator. Answered by the channel itself |
| `control.pending` | — | see [Limits](#limits) |
| `command.run` | `command`: a command id (`facade.place`), `params`: its parameters | the command's result. Runs exactly as the owner's commands do: journaled, undoable, refused changes leave the model as it was. Listed in the feed under the client's name, and the rebuild after a change highlights what it changed |
| `commands.list` | `schemas?`: bool | every command: `id`, `summary`, `changes_model`, and with `schemas` its `params_schema` |
| `document.summary` | — | `document.info`, plus `path` and `unsaved` |
| `ui.view` | `mode?`: `3d`, `plan` or `drawing`; `drawing?`: saved drawing view id (implies drawing mode); `level?`: a level index (the plan's level; in 3D, show up to that level, cut 1.2 m above its floor), or `null` for the whole model in 3D; `phase?`: a phase id; `filter?`: `complete`, `demolition`, `new` or `all`; `appearance?`: `materials` (default actual finishes) or `phases` (3D phase colors); `fit?`: bool; `focus?`: an element id (a facade's wall such as `b1.f3`, a placement such as `b1.f3.L2.B0`), framed and seen from outside its facade | the view: `mode`, `drawing`, `drawings`, `drawing_mapping`, `level`, `plan_level`, `levels`, `phase`, `phases`, `filter`, `appearance`, `selection`, `camera`, `model_box_m`, `window_hidden`. The parameters are checked whole before anything changes |
| `ui.click` | `x`, `y`: window points from the top left; `button?`: `left` (default), `right`, `middle`; `double?`; `shift?`, `command?`, `alt?`, `ctrl?` | `{queued, window_hidden}`. Real pointer input: the events a click makes (move, press, release) are fed to the window one per frame, so they act exactly as the owner's click (picking an element, pressing a button). Nothing arrives while the window is hidden |
| `ui.key` | `key`: an egui key name (`K`, `Escape`, `Enter`, `Z`…); `command?`, `shift?`, `alt?`, `ctrl?` | `{queued, window_hidden}`. A real key press and release (`{"key": "K", "command": true}` opens the command palette) |
| `ui.drag` | `from`, `to`: `[x,y]` in window points; `steps?`: integer 1–60 (default 8); the same optional modifiers as `ui.click` | `{queued, window_hidden}`. Primary-button press, bounded intermediate moves, and release, delivered one event per frame. The entire request is checked before input is queued. |
| `ui.screenshot` | `target?`: `viewport` (default) or `window`; `path?`: a `.png` file to write; `raise?`: bool | `{width, height, target}` plus `path`, or `png_base64` when no path is given. Answered once the view shows the latest change (a rebuild in progress is waited for). `viewport` is drawn offscreen at the viewport's size (the 3D view on the GPU, the plan with resvg) and works with the window covered. `window` captures the whole window with the activity feed; a minimised or covered window draws nothing, so the request fails at once unless `raise: true` brings the window to the front first (it takes focus from the owner: only when they asked for it). Window captures wider than 2400 pixels are halved |

Any other method is answered `unknown method`, with the list.

`ui.view` also returns `viewport_window_points: [x,y,width,height]` and `handles`, the currently
available dimensional handles for the selected element. Each handle gives its `key`, `label`,
`point: [x,y]` in window points, and `value_mm`. These are the same projected markers drawn by the
window; stale, historical, ambiguous or noneditable selections expose no handles. Use `ui.drag`
to operate one through the same input path as a pointer, then read the refreshed source properties.
`window_focused` reports the native focus flag (or null if the platform has not supplied it).
Handle drags cancel on lost focus; an explicitly requested window screenshot with `raise: true`
requests focus even if the window reports that it is already unoccluded.

## Example

```text
{"id":1,"method":"control.hello","params":{"client":"my-script"},"token":"<hex from the token file>"}
{"id":2,"method":"command.run","params":{"command":"facade.list"}}
{"id":3,"method":"ui.view","params":{"mode":"3d","focus":"b1.f30"}}
{"id":4,"method":"command.run","params":{"command":"facade.place","params":{"facade":"b1.f30","type":"window.casement.2","levels":[2,3]}}}
{"id":5,"method":"ui.screenshot","params":{"target":"window","path":"/tmp/after.png"}}
{"id":6,"method":"command.run","params":{"command":"edit.undo"}}
```

## MCP

`trigon mcp` serves MCP (JSON-RPC 2.0) on stdio. Bridged to Studio, every tool call becomes a
`command.run` on this channel, the MCP client's name (from `initialize`) is sent with
`control.hello`, and two more tools appear: `studio_view` (`ui.view`) and `studio_screenshot`
(`ui.screenshot`, returned to the agent as an image). A call whose connection breaks is not sent
again, since it may have run; the next call reconnects, also to a restarted Studio.

## Security notes

- Loopback only, a fresh token per launch in a user-only file, and nothing runs before the token
  is shown. Other users of the same machine can't drive the window.
- A client holding the token can do what the owner can, including reading and writing files
  through commands (`document.save`, exports). Polykrom's `allow_paths` folder limits are not
  implemented yet.
- Credentials never pass through the channel: data providers read them from Studio's environment.


Saved drawing workspaces use the same annotation commands as headless sessions; see
[annotations](annotations.md). In drawing mode, `ui.view` returns `drawing_mapping` once ready:
`origin` is drawing [0,0] in window points, `points_per_mm` is the positive scale and `viewport`
is [left, top, width, height]. Map drawing [x,y] to [origin.x + x*scale, origin.y - y*scale].
Pan/zoom changes this mapping, so read it again before synthetic point picking. A failed or
pending drawing does not expose an old transform.

The 3D appearance choice is ephemeral UI state: it does not alter the model, its journal, phase visibility, schedules or phase status. Materials preserve the existing demolition cue. Phase colors explicitly highlight new work.

For native drawing performance investigation, `ui.view` includes `drawing_raster` upload/discard
counters and last CPU allocation/render/conversion/texture-enqueue times. GPU upload and frame
pacing require native profiling; the response sets `gpu_upload_measured: false`.
