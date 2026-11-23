# Contributing to snip

Thanks for taking a look. snip is a small, Windows-only Rust project; issues and pull requests are welcome.

## Build and test

You need Windows 11, Rust 1.97 or newer and the MSVC toolchain with the Windows SDK (`rc.exe`; set `RC` when it is not on `PATH`).

```powershell
cargo build --release
cargo test
cargo clippy --all-targets          # please keep it free of warnings
target\release\snip.exe --selftest  # headless checks, exit 0 = all passed
```

`snip --selftest-overlay` opens real overlay windows, so it refuses to run on the visible desktop. Only run it on a separate, non-default desktop.

## Ground rules

- **Keyboard hook rules are guarded by tests.** The hook decides in `src/hookstate.rs` (a pure state machine). A hook that swallows the wrong key breaks typing on the whole machine, so change a rule only together with its test.
- **Every shortcut snip does not own must keep passing through.** snip takes only its configured keys, and Esc/Enter/arrows while its overlay is open.
- **Saved formats stay loadable.** `config.toml`: every key optional, unknown keys are an error; give every new key a default. `state.json`: bump `"schema"` when its shape changes.
- **No new runtime dependencies without a reason.** PNG encoding uses WIC and the clipboard uses Win32 directly.
- Keep `cargo fmt`-style formatting and the existing comment style: say *why*, briefly.

## Pull requests

1. Fork, create a branch from `main`, keep the change focused.
2. Run the commands above and say in the PR what you ran and on which Windows version.
3. For anything visible (overlay, tray), a short screenshot or GIF helps.

Please be kind and patient in issues and reviews. I maintain this on my own, in my spare time.
