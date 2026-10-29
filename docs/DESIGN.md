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

### Tier 1 — recipes (MVP)
Curated, tested, multi-step tasks. Built from primitives.

```yaml
id: device.grant_hid_browser_access
version: 1
summary: "Let a Chromium-based browser access a HID device (e.g. keyboard configurators)"
domains: [input_devices]
patterns:
  - "give {browser} access to (my )?{device}"
  - "let {browser} (configure|access) (my )?{device}"
slots:
  device:  {type: hid_device}
  browser: {type: browser, filter: {webhid: true}}
requires: {profile: {init: systemd}}
risk: medium
steps:
  - primitive: file.write_root
    args:
      path: "/etc/udev/rules.d/70-sancho-{device.vid}-{device.pid}.rules"
      content: 'KERNEL=="hidraw*", ATTRS{idVendor}=="{device.vid}", ATTRS{idProduct}=="{device.pid}", TAG+="uaccess"'
    explain: "Allow your user session to access {device.name} ({device.vid}:{device.pid})"
  - primitive: udevadm.reload_trigger
    explain: "Reload device rules so the change applies now"
notes:
  - "WebHID only works in Chromium-based browsers; warn Firefox users before planning."
tests:
  verified_by: mock      # udev isn't meaningful in containers
  reason: "Requires real hidraw devices; resolver tested against fixtures"
```

### Tier 2 — command primitives (MVP: schemas used by recipes and `explain`; standalone
generation is post-MVP)

```yaml
primitive: systemctl.enable
command: [systemctl, enable]
package: systemd
needs_root: true
args:
  unit: {type: systemd_unit}
flags:
  --now: {effect: service_runtime}
risk: medium
effects:
  - {kind: service_enablement, target: "{unit}"}
  - {kind: service_runtime, target: "{unit}", if_flag: --now}
read_only: false
dry_run: null
```

`read_only: true` primitives (e.g. `lsusb`, `systemctl status`, `ip link`) may run
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
| `path` | filesystem | Existence/permission checks |

Candidates are given opaque IDs (`d1`, `d2`). A selector (patterns/rules now, a model
later) may only return a candidate ID or `ask_user`.

## 5. System profile

Detected on first run, cached, refreshed on demand or when stale:
distro (Arch / Artix / derivatives like Omarchy), init (systemd/openrc/runit), network
stack (NetworkManager/iwd/systemd-networkd/connman), audio (PipeWire/PulseAudio),
display server (X11/Wayland), AUR helper (paru/yay/none), root filesystem (btrfs →
snapshots available), snapshot tool (snapper/timeshift), bootloader.
Recipes declare requirements; mismatches exclude the recipe from matching.

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
| `runtime` (reloads, module reloads) | — | — | no undo needed |
| destructive (rm, mkfs, dd) | — | — | irreversible (recipes use `gio trash` instead of rm) |

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

`eval/requests.yaml`: NL request → expected recipe + slots (or `clarify`).
Metrics: intent accuracy, slot accuracy, **wrong-plan rate** (critical), clarification rate,
latency, total RAM.
Recipe verification layers: podman containers (packages/files), QEMU VMs (systemd/boot/
kernel, post-MVP), fixture-based mocks (hardware resolvers), volunteer hardware testers.

## 14. Licensing

Decided: code **and** recipe/primitive data are dual-licensed `MIT OR Apache-2.0`
(`LICENSE-MIT`, `LICENSE-APACHE`); both include the no-warranty statement. Contributor
consent text for shared recipes must reference this license.

Still to decide: DCO sign-off; whether recipe data should move to a separate data license.
