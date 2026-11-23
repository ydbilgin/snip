# Security policy

## Supported versions

Only the latest release receives fixes.

## Reporting a vulnerability

Please **do not open a public issue**. Report it privately through GitHub's private vulnerability reporting:
<https://github.com/ydbilgin/snip/security/advisories/new>

Include what you found, how to reproduce it and the snip version (`snip --version`). I maintain snip on my own, so please allow some days for an answer.

## What snip touches

- **Keyboard:** a low-level keyboard hook (`WH_KEYBOARD_LL`) in its own process. It is not injected into other processes. It acts only on snip's configured keys and, while a snip overlay is open, on Esc, Enter and the arrow keys; it does not record keystrokes.
- **Screen:** it captures the monitors only when you start a capture or a recording.
- **Clipboard:** it writes pictures, text and recordings. It reads the clipboard sequence number to avoid overwriting a newer owner, and reads back its own text write to confirm it.
- **Files:** pictures and recordings in your Screenshots folder (or the configured `folder`); the config in `%APPDATA%\Laureth\snip\`; log, state, history pictures and the last 20 copied texts in `%LOCALAPPDATA%\Laureth\snip\`. The log holds step timings and the paths of saved files, never screen contents or clipboard text. History can be turned off in Settings.
- **Network:** none. snip has no network code.
- **Privileges:** the exe runs as the current user (`asInvoker`). Only `install.ps1 -RunAtLogon`, run by an administrator, registers a scheduled task that starts it at the highest level, from `%ProgramFiles%\snip`. Programs snip opens from there are started with the shell's normal-level token. The elevated tray window accepts a few messages from normal-level processes (close, its command message, the settings reload and Explorer's TaskbarCreated) so that `snip quit`, `snip snip` and the Settings window keep working.

## Known limits

- When snip runs elevated, its settings, log and history stay in the user's profile, which normal-level programs can also change. A program already running as you could, for example, point the screenshot folder somewhere else. Run snip at the normal level if that matters more to you than the keys above elevated windows.
- Any program on the same desktop can send snip its command message (for example "take an all-screens capture"), as with most tray programs.
