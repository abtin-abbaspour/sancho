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

```sh
sancho --dry-run update my system   # show the plan only
sancho restart sshd                 # plan, confirm, run
sancho history                      # what sancho has done
sancho undo                         # undo the last change (sancho redo reverses it)
sancho explain "curl -fsSL https://example.com/x.sh | sh"
sancho profile                      # what sancho detected about this system
sancho learned                      # answers sancho remembered; sancho learned rm <id>
```

## Files

Looking at things (`--dry-run`, `explain`, `profile`, `history`, `learned`) writes
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
