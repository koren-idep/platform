# Integrated terminal

Koren Code Editor provides an interactive local shell in its resizable bottom panel. The reusable `koren-terminal` crate owns PTY lifecycle and byte I/O; the desktop host owns user approval and IPC; xterm.js renders the terminal in the WebView. The service crate has no dependency on the editor, language tools, or extension runtime.

## Shell profile and authorization

Opening the Terminal panel does not start a shell. Selecting **New** creates a pending request and shows a graphical approval dialog. The dialog identifies the shell executable, direct argv, resolved working directory, environment override names, requesting actor, and associated workspace. Approval is one-time, expires after two minutes, and is invalidated if the active workspace changes. Denial, Escape, and expiration consume the pending shell without spawning it.

The default profile detects `SHELL` on Unix and `COMSPEC` on Windows. It falls back to `/bin/sh` or `cmd.exe` if the detected executable cannot start. It starts in the workspace root when a workspace is open and otherwise uses the user's home directory. Custom profiles can define display name, executable, argv, working-directory behavior, and explicit environment overrides. Argument values are passed directly and are never interpolated into an implicit command string. Profile environment values are redacted from debug and approval summaries. Profiles and workspace metadata never run commands automatically; shell startup files are left untouched.

An approved shell can run arbitrary commands with the user's account permissions. Approval authorizes an interactive shell session, not each command typed later, and the terminal is not a filesystem sandbox. Extensions have no terminal host operations and receive no terminal output events. Terminal output is not logged by the application.

## Lifecycle and bounded I/O

The lifecycle is `Created → Starting → Running → Exited | Failed | Terminated`. Creating a session reserves an ID but does not allocate a PTY or start worker threads. The default service limit is eight sessions; public configuration is capped at 32 sessions, 128 output chunks, and 64 input chunks per session. Closing a running tab or a finished shell whose owned process group still has live processes asks the user before termination. A 150 ms graceful window is followed by a forced kill where supported. Host shutdown also terminates and releases owned sessions.

PTY input and output are byte streams, not UTF-8 text. Typed requests carry base64 input; desktop events carry base64 output plus a per-session sequence number. The WebView decodes output as bytes and xterm handles terminal encoding and escape sequences. Input is limited to 64 KiB per request. Reader messages are at most 16 KiB; per-session output queues and input queues are bounded; a full input queue returns a retryable backpressure error. Each host drain delivers at most 128 KiB. Each desktop event subscriber has a two-batch channel and one additional bounded pending batch; a stalled subscriber pauses service drains, so backpressure reaches the PTY reader rather than silently dropping terminal output.

The terminal uses xterm.js 6 and its fit addon 0.11. It uses xterm's built-in renderer and does not enable WebGL or hyperlink addons. It is lazy-loaded on first panel use. Selection copy, paste, and display clear use Ctrl+Shift+C, Ctrl+Shift+V, and Ctrl+L while the terminal has focus. Ctrl+grave toggles the bottom panel with Terminal selected. Clipboard access requires the explicit shortcut gesture; arbitrary clipboard writes, terminal-to-IPC command invocation, HTML injection, and automatic command execution are not supported.

## Platform limits and verification

`portable-pty` 0.9 is used rather than a custom PTY implementation because it provides one maintained Rust interface for Unix PTYs and Windows ConPTY. On Unix, the service signals the owned process group with SIGTERM and then SIGKILL after the grace period. On Windows, the service can terminate the ConPTY child but does not claim to kill every descendant process. The terminal feature has been compiled and interactively exercised with controlled PTY fixtures on Linux; GTK/WebKitGTK visual and clipboard behavior still requires a desktop acceptance run. Windows and macOS PTY behavior must be validated on those native systems before support is claimed. Linux is the verified desktop target for this phase.

For local frontend setup, `npm ci --prefix apps/koren-code/frontend` installs the pinned xterm packages. The terminal module and stylesheet are loaded only when the panel is opened. Rust service tests use the current test binary as a controlled child process so they do not depend on Bash or a specific user's shell.
