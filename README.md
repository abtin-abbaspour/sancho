# sancho

Plain-language Arch Linux tasks, done reliably: `sancho update my system`,
`sancho let chromium configure my keyboard`, `sancho restart wifi`.

sancho maps a request to a **verified recipe**, fills it with values read from your
system, shows the exact commands as a plan, and runs nothing until you confirm. Every
change is journaled so it can be undone. No model writes commands, nothing leaves your
machine, and it never runs as root (it asks `sudo` for each step that needs it).

> Early development (0.0.x). Try requests with `--dry-run` first.

## Install

From this repository (Arch Linux):

```sh
cd packaging/sancho-git
makepkg -si
```

`sancho-git` builds the latest commit on `main`; rebuild the same way to update.

## Use

Run `sancho` and talk to it:

```text
$ sancho
sancho 0.0.1 — tell me what you want done. Type "help" for examples, "exit" to leave.

sancho> update my system
  (shows the plan; nothing runs until you confirm)
sancho> is my wifi card realtek or intel
sancho> what does pacman -Syu do
sancho> what did you do
sancho> undo that
sancho> exit
```

`sancho --dry-run` starts a practice session: plans are shown, never run.
Up and down arrows recall earlier lines in the session (kept in memory only).

Every phrase also works as a one-off command, e.g. `sancho restart sshd`, and there are
direct commands for scripts: `sancho history`, `sancho undo [--last N | --since 2h | words]`,
`sancho redo`, `sancho explain "<command>"`, `sancho profile`, `sancho learned [rm|export]`.

## Files

Looking at things (dry runs, explanations, `profile`, `history`, `learned`) writes
nothing. sancho only writes:

| Path | When |
|---|---|
| `~/.local/state/sancho/journal/` | when a plan runs (history and undo) |
| `~/.local/share/sancho/learned.json` | when you answer one of its questions |
| `/var/lib/sancho/backups/` (root-only) | when a step replaces a file (for undo) |

Plus whatever a recipe you confirmed changes (e.g. a udev rule in
`/etc/udev/rules.d/70-sancho-*.rules`); `sancho undo` reverts those.

## Uninstall

1. Optional: `sancho undo` the changes you don't want to keep (`sancho history` lists them).
2. `sudo pacman -R sancho-git`
3. Optional, to remove its data too:

   ```sh
   sudo rm -r /var/lib/sancho
   rm -r ~/.local/state/sancho ~/.local/share/sancho
   ```

## Development

See `docs/DESIGN.md` and `docs/MVP_PLAN.md`.
`scripts/ci.sh` runs formatting, lints, tests and the matching eval.

## License

MIT OR Apache-2.0.
