# sancho — Design

## 1. Positioning

**Runs on anything, and you can trust what it does.**

Existing natural-language shell tools (whatisit, larpshell, zx-ai, shell-gpt, pls) are
generation-first: a model writes the command and safety is a denylist or risk score bolted
on afterwards. Their reliability *is* their model, so they need large models (≈1 GB local,
or cloud).

sancho inverts this. Reliability lives in a verified recipe library, typed slot resolvers
that read the real system, schema validation, and a state-capturing journal. The model's
job shrinks to *choosing among options*, which is why the default install fits in < 100 MB
and runs offline on old laptops, Raspberry Pis, GPU-less servers and air-gapped machines.

The durable advantage is the reliability architecture and the recipe library; small size
is the visible consequence. The library and validator should eventually be usable by other
tools and agents (CLI query interface, MCP server).

## 2. Request pipeline

```
request
  -> normalize (lowercase, synonyms, typo correction against library vocabulary)
  -> learned aliases (local, labeled)
  -> match: patterns + BM25 (+ embedding scorer, phase 2) over Tier 1 recipes
  -> slot resolution: code enumerates candidates from the system; matcher selects
  -> plan: steps, risk, reversibility class per step, preconditions checked
  -> confirmation (templated, with real values; read-only steps run without prompting)
  -> execute (argv, per-command sudo, PTY passthrough), journaling each step
  -> templated summary
```

Routing falls through tiers only when the higher tier can't serve the request. The user
always sees which tier produced the plan: **verified recipe** / **checked command** /
**unverified**.

Clarification is decided by code, not a model: ask when the top two matches are within a
margin, when a required slot has 0 or >1 candidates after filtering, or when confidence is
below threshold.

## 3. Tiers

### Data files
Recipes and primitives are TOML (`data/recipes/*.toml`, `data/primitives/*.toml`), one
item per file, file name = id. TOML is chosen over YAML for explicit typing (no `on` →
`true` surprises), no anchors/aliases, and a small, well-maintained parser. Unknown keys
are errors; every problem is reported with file, line/column or field path, and reason.

IDs are `group.action` (`systemctl.enable`, `device.grant_hid_browser_access`).

**Templates.** Strings may contain `{name}` or `{name.attr}` placeholders; `{{`/`}}` are
literal braces. Filled values become single argv elements, stdin, or display text —
never shell input.

### Tier 1 — recipes (MVP)
Curated, tested, multi-step tasks. Built from primitives.

```toml
id = "device.grant_hid_browser_access"
version = 1
summary = "Let a Chromium-based browser access a HID device (e.g. keyboard configurators)"
domains = ["input_devices"]
patterns = [
  "give {browser} access to (my )?{device}",
  "let {browser} (configure|access) (my )?{device}",
]
risk = "medium"
notes = ["WebHID only works in Chromium-based browsers; warn Firefox users before planning."]

[slots]
device = { type = "hid_device" }
browser = { type = "browser", filter = { webhid = true } }

[requires]            # profile facts; each lists the accepted values
init = ["systemd"]

[[steps]]
primitive = "file.write_root"
explain = "Allow your user session to access {device.name} ({device.vid}:{device.pid})"
[steps.args]
mode = "0644"
path = "/etc/udev/rules.d/70-sancho-{device.vid}-{device.pid}.rules"
content = 'KERNEL=="hidraw*", ATTRS{{idVendor}}=="{device.vid}", ATTRS{{idProduct}}=="{device.pid}", TAG+="uaccess"'

[[steps]]
primitive = "udevadm.reload"
explain = "Reload device rules"

[[steps]]
primitive = "udevadm.trigger"
args = { subsystem = "hidraw" }
explain = "Apply the new rule to {device.name} now"

[tests]
verified_by = "mock"  # udev isn't meaningful in containers
reason = "Requires real hidraw devices; resolver tested against fixtures"
```

Rules enforced at load time:
- Every step supplies every arg of its primitive.
- Args of system-resolved types (`systemd_unit`, `package`, `hid_device`, …) must be
  filled by exactly one slot of the same type — never a literal or a composed string.
- Literals are checked against the arg type (`path` absolute without `..`, `file_mode`
  octal, `count` integer, `choice` in `values`). A composed `path` must start with a
  literal absolute prefix.
- Compound slots (`hid_device`, `browser`) are referenced by attribute (`{device.vid}`).
- `{profile.<fact>}` may be used only if `requires` constrains that fact.
- A `multiple` slot may only fill a `variadic` arg.
- Recipe `risk` is at least the highest step primitive risk.
- Every slot is used by some step; `tests.reason` is required unless `verified_by = "container"`.

### Tier 2 — command primitives (MVP: schemas used by recipes and `explain`; standalone
generation is post-MVP)

A primitive is **one fixed command line** with typed holes. Variants (e.g. enable vs
enable-and-start) are separate primitives or separate recipe steps, so every step has a
fixed argv shape and a fixed set of effects — which keeps journaling and undo per step.

```toml
id = "systemctl.enable"
summary = "Enable a systemd unit so it starts at boot"
command = ["systemctl", "enable", "{unit}"]
package = "systemd"
needs_root = true
risk = "medium"
read_only = false
effects = [{ kind = "service_enablement", target = "{unit}" }]

[args]
unit = { type = "systemd_unit", help = "Unit to enable" }
```

- `command` is the exact argv. Each arg appears in it exactly once as a whole-token
  placeholder (`"x{unit}"` is rejected); the program name may only come from a `choice`
  arg (e.g. `{helper}` ∈ paru/yay).
- `variadic` args expand to one argv element per value. `text` args are passed on
  `stdin` and only there.
- `choice` values may not start with `-`, so they can never act as flags.
- `interactive = true` runs the command on a PTY. `dry_run` is the same command in the
  tool's show-only mode (e.g. `pacman --print`), using the same args.
- `read_only` primitives declare no effects; all others declare at least one.

`read_only = true` primitives (e.g. `lsusb`, `systemctl status`, `ip link`) may run
without a y/n prompt, but are always displayed.

### Tier 3 — free-form (post-MVP)
Model-generated, parsed to a bash AST, validated against any known primitives, never
batched, stronger confirmation for root, labeled **unverified**, logged locally as a
candidate recipe.

## 4. Slot types and resolvers

Each slot type has a resolver that enumerates candidates from the system and a validator.

| Type | Source | Notes |
|---|---|---|
| `hid_device` | `/sys/class/hidraw`, `udevadm info` | Filter by udev props: `ID_INPUT_KEYBOARD=1`, `ID_INPUT_MOUSE=1`, etc. "my keyboard" = property filter, not inference |
| `systemd_unit` | `systemctl list-unit-files` | Must exist |
| `package` | `pacman -Q` / `pacman -Si` | Installed vs available |
| `net_iface` | `ip -j link` | Wireless detection via `/sys/class/net/*/wireless` |
| `browser` | installed packages + .desktop files | Capability flags (e.g. `webhid`) |
| `user_group` | `/etc/group` | |
| `user` | current user, `/etc/passwd` | |

Literal types, checked at load and fill time: `path`, `file_mode`, `count`, `text`
(stdin only), `choice` (fixed `values`). Resolved values interpolated into templates
(e.g. `{device.vid}` in a path) are validated by their resolver to be safe there.

Candidates are given opaque IDs (`d1`, `d2`). A selector (patterns/rules now, a model
later) may only return a candidate ID or `ask_user`.

## 5. System profile

Nine facts, each with a fixed vocabulary (`ProfileKey::values()` in `sancho-core`), so
recipe `requires` values are checked at load time:

| Fact | Values | Evidence |
|---|---|---|
| `distro` | arch, artix, cachyos, endeavouros, garuda, manjaro, arch_derivative, unsupported | os-release `ID`, `ID_LIKE` |
| `init` | systemd, openrc, runit, dinit, s6, unknown | `/run/systemd/system` etc., then `/proc/1/comm` |
| `network` | networkmanager, connman, iwd, systemd_networkd, none | running daemons (NM/ConnMan win over iwd, their usual Wi-Fi backend) |
| `audio` | pipewire, pulseaudio, none | running daemons |
| `display` | wayland, x11, none | `XDG_SESSION_TYPE`, then `WAYLAND_DISPLAY` / `DISPLAY` |
| `aur_helper` | paru, yay, none | executable on `PATH` |
| `root_fs` | btrfs, ext4, xfs, f2fs, zfs, bcachefs, other | last `/` entry in `/proc/self/mounts` |
| `snapshot_tool` | snapper, timeshift, none | installed **and** configured for `/` |
| `bootloader` | systemd_boot, limine, refind, grub, unknown | ESP/`/boot` files, then `/etc/default/*` |

Detection reads files only (never runs commands), takes well under a millisecond, and
runs fresh on every invocation. It is not cached: a cache goes stale (the display differs
between SSH and desktop; packages change) and stale facts would produce wrong plans.
Recipes declare `requires`; any unmet requirement excludes the recipe from matching, with
a reason ("needs init systemd, this system has openrc").

## 6. Execution

- Refuse to run as root. Per-command `sudo` with the exact argv displayed.
- argv only; no shell interpolation of slot values.
- PTY passthrough so package-manager questions reach the user (no blanket `--noconfirm`).
- Ctrl-C / timeout / failure: stop, mark the transaction, offer rollback of completed
  steps (all-or-nothing behaviour for multi-step recipes).
- AUR helpers must run as the user, never via sudo.

## 7. Journal and undo

### Principle
Undo **restores captured state**. Before each step, the effect handler captures the state
it will change; undo restores it. Never run a guessed inverse (e.g. don't `disable` a
service that was already enabled before we touched it).

### Effect handlers (MVP)

| Effect | Capture | Restore | Class |
|---|---|---|---|
| `file_write` / `file_edit` | copy + hash, or "absent" | restore copy / delete | reversible |
| `service_enablement` | `is-enabled` | set back | reversible |
| `service_runtime` | `is-active` | set back | reversible |
| `package_install` / `package_remove` | package set + versions | remove added / reinstall from cache | reversible while cached |
| `group_membership` | user's groups | set back | reversible |
| `radio_state` | radio on/off | set back | reversible |
| `package_upgrade` | snapshot, if a snapshot tool exists | guided snapshot rollback | snapshot only |
| `package_cache_prune` | — | — | irreversible (invalidates package undo) |
| `runtime` (restarts, reloads, triggers) | — | — | no undo needed |
| `destructive` (rm, mkfs, dd) | — | — | irreversible (recipes use `gio trash` instead of rm) |

System upgrades: downgrading individual packages is a partial upgrade (unsupported on
Arch). Undo = filesystem snapshot (snapper/timeshift) taken before the upgrade, with
guided rollback instructions; without snapshots, the plan states it cannot be undone.
`paccache` cleanup can invalidate package undo; affected journal entries are re-marked.

### Storage
`~/.local/state/sancho/journal/<tx_id>/manifest.json`; root-owned backups in
`/var/lib/sancho/backups/` (root-only perms). Bounded by age and size.

Manifest records: request text, tier, recipe id+version, resolved slots, per-step argv,
exit code, effect, pre-state, post-state hash, reversibility class, plus a **human summary
and entity tags** (device names, services, packages, paths, domains) used by NL undo.

### Undo semantics
- Default unit is the **transaction** (one request). Partial (step-level) undo allowed when named.
- Undo plans run last-to-first, are previewed, confirmed, and journaled (so they can be redone).
- **Drift:** if current state ≠ recorded post-state, show the difference and ask.
- **Dependencies:** transactions touching the same resource (file, unit, package, group,
  device) are linked. Undoing T when a later transaction depends on it → offer
  [undo both] / [undo only T, with consequence] / [cancel]. Never cascade silently.
- Mixed reversibility in a range → split the preview into "can undo" / "can't undo".

### NL undo modes
`last` ("undo", "reverse that") · `select` ("undo the keyboard thing") · `range`
("undo the last 3", "everything since this morning" — time parsed by code) · `partial`
("keep the rule but undo the group change") · `redo` · `by_symptom` ("undo whatever broke
my wifi" — rank by domain overlap, always present as a choice, offer troubleshooting if
nothing matches). Pronouns resolve to the current session first.

## 8. Explain

`sancho explain "<command>"` and NL forms ("what does X do"):
- X is a command string → parse with tree-sitter-bash, explain each command/flag from
  primitive schemas, show risk. Deterministic, no model. Useful for commands copied from
  forums/wikis.
- X is a tool/concept ("what does pacman do") → local docs: bundled tldr-pages
  (respect its license/attribution) + system man pages.
- Open-ended tutoring → bring-your-own-model (post-MVP).
- "help me with X" is ambiguous → ask: explain, fix, or do?

## 9. Diagnose-and-fix recipes (post-MVP, design now)

A recipe type with `symptoms`, read-only `diagnostics`, `fixes`, `prevention`. Piped
errors (`cmd 2>&1 | sancho fix`) match symptoms deterministically. Successful fix
sessions can be saved as learned playbooks.

## 10. Learning and sharing

- Local learned store, separate from shipped recipes: aliases (from clarification answers),
  slot preferences, saved recipes/playbooks. Every use is labeled in the plan. Never
  bypasses confirmation. `sancho learned list|rm|edit`.
- Local run counts only (gate the "share?" offer after N successful runs). **No telemetry.**
- Sharing is per item, opt-in, with a full preview of the exact payload after scrubbing
  (usernames, hostnames, home paths, IPs, MACs, SSIDs, serials). Error output/logs need
  the most scrubbing. MVP: export to a file; later: pre-filled GitHub issue.
- Share prompt includes license consent for the recipe-data license.
- Submissions are untrusted: container/VM tests + human review, never auto-merged.
  No importing others' learning packs.
- Global trust signal = **verification**, not popularity: "tested in CI on profiles X, Y,
  last verified <date>".

## 11. Models

Model-agnostic by contract: input = request + candidate lists; output = constrained JSON
(`{recipe, slots: {name: candidate_id | "ask_user"}}`). Larger models may handle harder
requests but never bypass recipes or validation.

- **Lite (default, < 100 MB):** rules + BM25 + a small model that selects recipes and
  slot candidates. Model architecture to be decided by bake-off. Explanations are templates.
- **Standard (optional):** small fine-tuned generative model for grounded explanations,
  paraphrase/compound requests, Tier 2 generation under grammar constraints. Explanations
  checked against a fact sheet (every path/ID/name must appear in it, else fall back to template).
- **Bring-your-own-model:** local (Ollama etc.) or cloud API for complex flows,
  troubleshooting, web search. Strict JSON validation (APIs may lack grammar constraints).
  Users are told what system details a cloud backend receives; scrub where possible.

Model selection is by bake-off on `eval/`, not public benchmarks. Measure calibration;
don't trust raw probabilities.

## 12. Security

- Never run as root; per-command sudo; argv only.
- Recipe packs are effectively root code: design the loader for signature verification
  (implemented post-MVP).
- Command output, man pages, web content, community submissions = data, never instructions.
- Root-owned backups never copied into the user's home.

## 13. Evaluation

`eval/requests.toml`: NL request → expected recipe + slots (or `clarify`).
Metrics: intent accuracy, slot accuracy, **wrong-plan rate** (critical), clarification rate,
latency, total RAM.
Recipe verification layers: podman containers (packages/files), QEMU VMs (systemd/boot/
kernel, post-MVP), fixture-based mocks (hardware resolvers), volunteer hardware testers.

## 14. Licensing

Decided: code **and** recipe/primitive data are dual-licensed `MIT OR Apache-2.0`
(`LICENSE-MIT`, `LICENSE-APACHE`); both include the no-warranty statement. Contributor
consent text for shared recipes must reference this license.

Still to decide: DCO sign-off; whether recipe data should move to a separate data license.
