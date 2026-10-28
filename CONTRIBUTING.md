# Contributing to Better Finder

Thank you for your help. This page tells you how to build the app, how to test a change, and which rules the code follows.

## Build and test

You need macOS, a stable Rust toolchain, and the Xcode Command Line Tools.

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release -p bf-app -- ~/Downloads
```

The CI runs the same checks on each pull request. A pull request must pass them.

## Where the code is

| Path | What it holds |
| --- | --- |
| `crates/bf-core` | Pure Rust: config parser, keymap, directory listing, sort, filter, navigation state, scroll math. No UI code. Unit tests cover it. |
| `crates/bf-app` | The macOS app: AppKit through `objc2`. |
| `docs/ARCHITECTURE.md` | The design contract: goals, performance targets, flows, decisions. |
| `docs/UI.md` | The UI contract: layout, theme, keys, and the behavior of each element. |

Read the two docs before a large change. When a change alters a contract, update the doc in the same pull request.

## Rules for the code

- **Speed is the product.** The main thread never waits for the disk or for a process. Directory reads, path checks, volume lists, and ejects run on background threads.
- **Keep `bf-core` free of UI code.** Put all logic that a test can cover in `bf-core`, and add the tests.
- **Every feature is an action.** A new behavior gets an `Action`, a name in the catalog, and (when it makes sense) a default key. The user can rebind every key in the config file.
- **Data safety.** A rename never replaces an item. A delete goes to the Trash. A destructive action never runs on key repeat.
- **Re-entrancy.** In `bf-app`, borrow the app state, compute, drop the borrow, and only then call AppKit. See the notes at the top of `app.rs`.
- **Plain language.** Code comments and docs use ASD-STE100 Simplified Technical English: active voice, simple present tense, short sentences.

## Test the UI without a mouse

The binary has developer flags. `better-finder --help` lists them.

```sh
# Feed keys, then save a screenshot of the window and quit.
BETTER_FINDER_SCREENCAPTURE=own target/release/better-finder \
  --debug-keys "j j ctrl+d @load" --debug-screenshot /tmp/shot.png ~/Downloads

# Print timings: startup, loads, frames.
BETTER_FINDER_TRACE=1 target/release/better-finder ~/Downloads
```

For tests with disks, use disk images: `hdiutil create -size 20m -fs APFS -volname Test /tmp/test.dmg`, then `hdiutil attach /tmp/test.dmg`.

For tests of remote folders without a network, `BETTER_FINDER_SSH` replaces `/usr/bin/ssh`. This script skips the options of `ssh` and runs the command on this Mac:

```sh
#!/bin/sh
# fake-ssh: a "remote host" on this Mac, for UI tests.
while [ $# -gt 0 ]; do
  case $1 in --) shift; break ;; -o) shift 2 ;; -*) shift ;; *) break ;; esac
done
shift  # the destination
exec "$@"
```

Then add `remote = demo:/tmp/demo` to a test config (`XDG_CONFIG_HOME`), and run the app with `BETTER_FINDER_SSH=/path/to/fake-ssh`. Use a test folder, not your real files, for screenshots.

## Pull requests

1. Open an issue first for a large change, so that we can agree on the design.
2. Keep one topic in each pull request.
3. Describe what you changed and how you tested it. Add a screenshot for a UI change.
