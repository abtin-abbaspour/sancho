# sancho — MVP plan (v0.1)

Goal: a testable CLI that handles a small set of Arch tasks end to end — match, resolve,
plan, confirm, execute (in containers/mocks), journal, undo, explain — with **no
generative model**. Each milestone ends with passing tests and an updated eval report.

Work milestone by milestone. Don't start the next one until the current one's acceptance
criteria pass. Never run state-changing commands on the development machine:
recipes run in podman containers or through the mock executor.

---

## M0 — Scaffold
- Cargo workspace: core, profile, resolve, match, plan, exec, journal, explain, learn, cli.
- `sancho --version`, `sancho --help`; refuse to run as root.
- CI script: fmt, clippy, tests.

**Done when:** workspace builds, CI script passes, root refusal has a test.

## M1 — Core types and schemas
- Rust types + YAML loading for `Primitive`, `Recipe`, `Slot`, `Effect`, `Plan`.
- Schema validation with clear errors (file, field, reason).
- Write ~15 primitives: `pacman.sync_upgrade`, `pacman.install`, `pacman.remove`,
  `paccache.keep`, `aur_helper.upgrade`, `systemctl.enable`, `systemctl.disable`,
  `systemctl.restart`, `systemctl.status` (read-only), `file.write_root`,
  `udevadm.reload_trigger`, `usermod.add_group`, `lsusb` (read-only),
  `ip.link` (read-only), `nmcli.radio_wifi`.

**Done when:** all data files load; invalid fixtures produce precise errors.

## M2 — System profile
- Detect: distro, init, network stack, audio, display server, AUR helper, root fs,
  snapshot tool.
- Cache to `~/.local/state/sancho/profile.json`; `sancho profile` prints it.
- Tests use fixture filesystems / command outputs, not the host.

**Done when:** profile detection is covered by fixture tests for at least: vanilla Arch +
NetworkManager, Arch + iwd, Artix + OpenRC (recipe exclusion works).

## M3 — Resolvers
- `hid_device` (udev property filters: keyboard/mouse), `systemd_unit`, `package`,
  `net_iface` (wireless detection), `browser` (with `webhid` capability), `user_group`.
- Each resolver: enumerate → candidate list with opaque IDs + display names; validate.
- Fixtures in `eval/fixtures/` captured from real machines (read-only commands only).

**Done when:** "my keyboard" with one keyboard → auto-selected; with two → `ask_user`
with both names; with a keyboard + mouse → mouse excluded.

## M4 — Matcher
- Normalization (synonyms, typo correction against library vocabulary).
- Pattern matching with slot capture; BM25 fallback over recipe summaries/patterns.
- Clarification policy: margin between top-2, missing/ambiguous slots, low score.
- `eval` binary: runs `eval/requests.yaml`, reports intent accuracy, slot accuracy,
  wrong-plan rate, clarification rate.
- Write ≥ 150 eval requests across the starter recipes, including paraphrases, typos,
  and requests that *should* clarify or not match.

**Done when:** wrong-plan rate is 0 on the eval set (wrong → clarify is acceptable;
clarify rate is reported, target < 20%).

## M5 — Starter recipes (~10)
`system.update` (pacman + AUR helper + `paccache -rk2`; snapshot first if available;
warn that upgrades aren't reversible without snapshots), `package.install`,
`package.remove`, `service.restart`, `service.enable`, `service.disable`,
`network.restart_wifi` (profile-aware: NetworkManager vs iwd), `device.grant_hid_browser_access`,
`user.add_to_group`, `disk.usage` (read-only).

**Done when:** each recipe has ≥ 3 eval requests and either a passing container test or
a declared `verified_by` with reason.

## M6 — Planner + confirmation UI
- Plan rendering: tier badge, steps with templated explanations filled with real values,
  risk, reversibility class per step, labeled learned items.
- Read-only steps run without prompting (but are shown).
- Risk tiers: normal y/n; high-risk requires typing `yes`.
- `--dry-run` prints the plan and exits.

**Done when:** snapshot tests of rendered plans for every starter recipe.

## M7 — Executor + journal + undo (most safety-critical; tests first)
- `Executor` trait; `MockExecutor` (records argv, simulates outputs/state);
  `ContainerExecutor` (podman); `SudoExecutor` (real, only used in container tests).
- Capture/restore handlers: file, service enablement/runtime, package set, group membership.
- Transaction manifests with summaries + entity tags; root-owned backups separated.
- Mid-transaction failure → offer rollback of completed steps.
- `sancho undo` modes: `last`, `select` (by entity/summary search), `range` (count/time),
  `partial`, `redo`. Drift detection; dependency linking with the 3-option prompt.
- `sancho history` lists transactions.

**Done when:** container tests cover: undo restores a pre-existing enabled service
correctly (doesn't disable it); file drift is detected; dependent-transaction prompt
appears; failed step triggers rollback offer; undo of undo works.

## M8 — Explain
- `sancho explain "<cmd>"` via tree-sitter-bash + primitive schemas: per-command and
  per-flag explanation + risk; unknown commands/flags clearly marked.
- NL routing for "what does X do" / "explain X"; "help me with X" → ask explain/fix/do.
- Tool/concept questions answered from bundled tldr-pages + local man pages.

**Done when:** explain covers all primitives' flags; unknown flags are flagged, never guessed.

## M9 — Learned store + export
- Aliases saved from clarification answers (labeled in plans), slot preferences.
- `sancho learned list|rm`.
- `sancho learned export <id>` → scrubbed file with full preview before writing.

**Done when:** scrubber tests cover usernames, hostnames, home paths, IPs, MACs, SSIDs.

## M10 — Packaging + measurement
- Release build; measure peak RSS and latency on the eval set; record in `docs/BENCH.md`.
- Draft `PKGBUILD` (do not publish).

**Done when:** peak RSS < 100 MB, median latency reported.

---

## Phase 2 (after MVP testing)
Small selection model behind a feature flag (architecture chosen by bake-off), re-run
eval and RAM benchmark · diagnose-and-fix recipes + `sancho fix` · Tier 2 standalone commands ·
bring-your-own-model backend · signed recipe packs · GitHub-issue sharing · QEMU VM tests ·
query interface / MCP server for other tools.

## Open decisions (ask the maintainer when reached)
YAML crate choice. (Licensing decided: `MIT OR Apache-2.0`, see DESIGN.md §14.)
