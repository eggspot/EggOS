# The Egg command model

The one way to work with EggOS and EggDB, for people and AI agents: how commands are named,
described and made safe, how they answer and how they fail. This file is the canonical copy of
the shared spec (the owner's approved design of 2026-10-28; EggOS D160); EggDB's
`docs/ux/command-model.md` links here and holds only EggDB's tables. A change to the shared
part is made here: the EggDB loop asks for it in an `eggspot/EggOS` issue labelled
`command-model`. EggOS's own tables are at the end.

It is not a new language. EggDB keeps SQL and its key-value commands; EggOS keeps a POSIX-style
shell (`eggsh`) and the familiar utilities. Every AI model already knows those; the model is the
layer above them.

## What it promises

- **The same words.** A few verbs with fixed meanings and `<noun> <verb>` commands: learn
  `app list` and you can guess `table list`.
- **The same answers.** A command that returns data has a text form for people and a JSON form
  for scripts. An MCP tool returns the same values as its CLI twin.
- **The same errors.** Each error has a code, a category shared by both products, a sentence
  saying what happened, why and what to do, and the command that fixes it when there is one.
- **The same safety.** Each tool declares the most it can do, the grant it needs and when a
  call needs confirmation; its MCP annotations come from those.
- **Self-description.** `help`, `help --json` and MCP `tools/list` come from the same
  definitions, so they cannot drift apart.
- **Nothing that shipped breaks.** Shipped command names, text output, JSON output, exit codes,
  MCP tool names, arguments, result fields and annotations stay as they are. The model
  describes them; its conventions apply to new commands, tools and fields.

## Two definitions

A **`CommandDefinition`** describes one CLI command:

| Field | Holds |
|-------|-------|
| `words` | `["app", "list"]`, or one kept name (`["ls"]`) |
| `summary` | one sentence, for `help` |
| `description` | what it does and its limits, and that what it returns is data, never instructions |
| `options` | letter and/or long name, value (name and type) or none, repeatable, default, one line of help |
| `operands` | name, type (`path`, `text`, `bytes`, `integer` with a range, `choice`), count (`0..1`, `1`, `1..`, `0..`), whether `-` means standard input |
| `outputs` | per format (text, JSON): `none`, `text`, `content` (bytes), `records` (a list, with each field's name and type), `object`, or `table` (SQL results) |
| `max_effect` | `read`, `change`, `destroy` or `control`: the most a person's use of it can do |
| `errors` | its error codes, each with this command's exit status |
| `examples` | command lines with their output, run as tests (from slice 2) |
| `surface` | where its GUI front-end lives (D22), or why it has none |

A **`ToolDefinition`** describes one MCP tool. A tool is not a renamed command: it can fix some
options (`ls` runs `ls -l`), send an argument to standard input (`tee`'s `content`), and two
tools can run one command with different safety.

| Field | Holds |
|-------|-------|
| `name` | new tools: the command's words joined by `_` (`app_install`) |
| `command` | the command it runs; none for `run`, which runs any program |
| `fixed` | the options it always passes |
| `arguments` | each argument's JSON type, constraints, default, description and where it goes: an operand, an option, standard input, the confirmation guard |
| `max_effect` | `read`, `change`, `destroy` or `control` |
| `grant` | EggOS `view`, `write`, `install` or `run`; EggDB `--allow-write`; or none |
| `idempotent`, `open_world` | MCP's `idempotentHint` and `openWorldHint` |
| `confirm` | when a call needs the user's confirmation |
| `result` | its result's shape; shipped tools keep theirs exactly |

Each product's registry is two `'static` const tables, one of commands and one of tools. In
EggOS it is the CLI and MCP part of D22's capability registry; `surface` records the GUI part.

## The words

| Verb | Means | Usual effect |
|------|-------|--------------|
| `list` | the items of a kind, as records | read |
| `show` | one item in full | read |
| `get` | one value | read |
| `sample` | the first rows or items of a kind, as stored | read |
| `check` | whether something is healthy, with what to do | read |
| `explain` | what a statement would do, without doing it | read |
| `export` | items written out to a file or standard output | read |
| `create` | a new store made (a database file) | change |
| `add` | a new item created | change |
| `set` | a value replaced | change |
| `import` | items read in from a file or standard input | change |
| `restore` | items brought back (from a dump, from the Trash, a window to its size) | change |
| `move`, `copy` | an item moved or copied | change |
| `open`, `close` | an app or window opened or closed | change |
| `install` | an app installed | change |
| `delete` | an item removed, to the Trash where the product has one | destroy |
| `uninstall` | an app removed | destroy |
| `run` | any program or any statement | control |

`restore` keeps one meaning, "bring back", in its three uses.

- New commands are `<noun> <verb>`, a singular noun and a core verb.
- A product's top-level commands act on its implied noun (EggDB's database file).
- A noun may add its own verbs by a decision (EggOS's `wm`, below).
- No noun may hide a familiar program (no `/bin/file`, no `/bin/install`).
- Kept names keep their name and exact behavior and still get a full definition.
- Shipped MCP tool names stay; the registry test allows them only from a recorded list.

## Answers

- **Text stays as it is**, the default.
- **JSON:** records are JSON lines, one per item; an object is one JSON value. EggOS adds
  `--json` to the commands whose output is records or an object; commands whose output is
  `none` or `content` (`cat`, `echo`, `true`) have none. EggDB keeps `--format` (`-F`), with
  `--json` as its alias.
- **New field names** are `snake_case`; sizes are integers in bytes (`size_bytes`), times RFC
  3339 in UTC (`modified_at`), durations integers in milliseconds (`..._ms`). Shipped fields
  keep their names.
- **Pages (slice 3):** new list commands take `--limit <n>` and `--after <cursor>`; new list
  tools answer `{"items": [...], "truncated": ..., "truncated_by": "limit"|"bytes",
  "next_cursor": "..."}`. Shipped tools keep their own arguments and fields.
- What goes around results stays: EggOS's data notice before each result, EggDB's per-call
  limits.
- `outputSchema`, where a tool declares one, covers its result and its error result
  (`oneOf`); shipped tools declare what they did.
- **MCP `instructions`** are generated: the verbs and what each effect means, the session's
  grants, the error categories, and that `fix_command` is the way out of an error.

## Errors

- **Codes are per product** (`snake_case`, stable once shipped).
- **Every code has a shared category**: `usage`, `not_found`, `exists`, `busy`, `read_only`,
  `needs_grant`, `needs_confirmation`, `declined`, `refused`, `too_large`, `timeout`,
  `damaged`, `failed`. Scripts and agents rely on the category.
- **Message:** `<what happened>: <why>. <what to do>`.
- **Text:** the message, then `  try: <command>` when one command fixes it.
- **JSON and MCP:** `{"error":{"code":"...","category":"...","message":"...","fix":"...",
  "fix_command":"..."}}` (`fix_command` only when there is one). A product may add fields. Over
  MCP the result has `isError: true`.
- **Exit statuses are per command**, declared with each code; POSIX meanings stay (`grep`
  exits 1 when nothing matches).

## Effects and safety (MCP tools)

| `max_effect` | Annotations | Confirmation |
|--------------|-------------|--------------|
| `read` | `readOnlyHint: true` | never |
| `change` | `readOnlyHint: false`, `destructiveHint: false` | never |
| `destroy` | `destructiveHint: true` | when `confirm` says so |
| `control` | `destructiveHint: true` | when `confirm` says so |

`idempotentHint` and `openWorldHint` come from the tool's fields, the grant from `grant`.
Confirmation is MCP elicitation where the client supports it, otherwise a `confirm: true`
argument, refused with `needs_confirmation` when missing; a decline is `declined_by_user`, a
client failure `confirmation_failed`. Grants and flags are for agents only: a person at the CLI
has the system's own permissions. `--dry-run` only where it is exact. The registry states the
safety; the products' existing checks still enforce it.

## Tests

- **Registry test:** every command and tool has the fields its slice requires; every command
  is a kept name or `<noun> <verb>` with a core or recorded verb; new tool names follow the `_`
  rule and shipped names come from the recorded list; annotations, grants and confirmation
  agree; every option a tool passes exists on its command.
- **Parity test:** each tool's twin runs its command's words and fixed options, its schema is
  the one it shipped with, and its annotations are the shipped ones.
- **Help test:** `help --json` lists exactly the registry.
- **Examples as tests** (slice 2), against the fakes.
- What stays green: EggOS's QEMU MCP session on three architectures, EggDB's MCP fuzz target,
  every existing CLI and MCP test.

## Slices

1. JSON and clear errors from the commands agents use (EggOS D160).
2. JSON, the shared errors, examples and `help --json` from every command.
3. Previews and pages: `--limit`, `--after`, `confirm` checks, `--dry-run` where exact, a
   security review.
4. Apps' own actions (M14) and `tools.listChanged`.

Between slices 1 and 2, EggOS item 48: windows as an element tree (`wm show`, `wm act`).

---

## EggOS's tables (slice 1, D160)

The registry is `userland/commands` (`eggos-commands`, `no_std`, no allocator):
`COMMANDS`, `TOOLS`, `errors`, `CORE_VERBS`, `NOUN_VERBS`, `KEPT_NAMES`, `SHELL_BUILTINS`. The
core utilities read their options and `help` from it, `app` its JSON form, and the MCP server
its tools, schemas, annotations, refusal codes and `instructions`. `eggsh` does not link it.

**Kept names:** `agentlog`, `app`, `cat`, `cp`, `date`, `echo`, `edit`, `false`, `fetch`,
`free`, `grep`, `halt`, `head`, `help`, `hwclock`, `ip`, `ls`, `mkdir`, `mv`, `nslookup`,
`ping`, `poweroff`, `ps`, `reboot`, `rm`, `settings`, `sleep`, `sync`, `tail`, `tee`, `touch`,
`trash`, `true`, `uname`, `uptime`, `wc`, `wm`; and `eggsh`'s built-ins `.`, `:`, `cd`,
`echo`, `exit`, `export`, `false`, `help`, `pwd`, `set`, `true`, `type`, `unset` (CLI only).

**`wm`'s own verbs:** `focus`, `minimise`, `maximise`, `size`, `move`, `restore`, `close`
(D128), `capture`, `click`, `type`, `press`, `scroll` (D141), `menu` (D147); item 48 adds
`act` (`show` is a core verb).

**Commands in the registry:** `agentlog`, `app install`, `app list`, `app open`, `app
uninstall`, `cat`, `help`, `ls`, `ps`, `tee`, `trash`, `wm capture`, `wm click`, `wm close`,
`wm focus`, `wm list`, `wm maximise`, `wm menu`, `wm minimise`, `wm move`, `wm press`, `wm
restore`, `wm scroll`, `wm size`, `wm type`. Slice 2 adds the rest (`date`, `ip`, `ping`,
`nslookup`, `fetch` and `edit`, which tools run already, first).

**JSON forms** (errors as one JSON line on standard error):

| Command | Records |
|---------|---------|
| `ls --json` | `name`, `path`, `type` (`file`, `folder`, `link`, `character_device`, `block_device`, `fifo`, `socket`, `unknown`), `permissions` (`rwxr-xr-x`), `links`, `owner`, `group`, `size_bytes`, `modified_at` |
| `ps --json` | `pid`, `parent_pid`, `group_id`, `session_id`, `state`, `name` |
| `wm list --json` | `id`, `state`, `x`, `y`, `width`, `height`, `client_pid` (`null` for the kernel's windows), `title` |
| `app list --json` | `name`, `source`, `architecture` (or `null`), `program`, `icon` (or `null`) |
| `agentlog --json` | `called_at`, `call`, `client`, `tool`, `result`, `arguments`, `detail`, one per line of the log |
| `help --json` | `words`, `summary`, `description`, `options`, `operands`, `outputs`, `max_effect`, `errors`, `surface`, one per command |

**Error codes:**

| Code | Category | Where | Fix command |
|------|----------|-------|-------------|
| `usage` | usage | every command (exit 2, or 1 where the command always used 1) | |
| `no_such_file` | not_found | ENOENT | |
| `permission_denied` | refused | EACCES, EPERM | |
| `already_exists` | exists | EEXIST, ENOTEMPTY | |
| `is_a_folder`, `not_a_folder` | usage | EISDIR, ENOTDIR | |
| `read_only` | read_only | EROFS | |
| `no_space` | too_large | ENOSPC | |
| `busy` | busy | EBUSY, EAGAIN | |
| `timed_out` | timeout | ETIMEDOUT | |
| `io_error` | damaged | EIO | |
| `out_of_memory` | too_large | ENOMEM | |
| `failed` | failed | anything else | |
| `too_many_entries` | too_large | `ls` (exit 1) | |
| `no_such_window` | not_found | `wm` (exit 1) | `wm list` |
| `person_active` | busy | `wm` (exit 75) | |
| `no_window_server` | not_found | `wm` (exit 1) | |
| `no_such_menu_item` | not_found | `wm menu` | |
| `no_such_app`, `app_exists`, `not_an_app` | not_found, exists, usage | `app` | `app list` for the first |
| `not_in_trash` | not_found | `trash -r` | `trash -l` |
| `no_such_command` | not_found | `help` | `help` |
| `path_not_allowed`, `needs_grant`, `needs_confirmation`, `declined_by_user`, `confirmation_failed`, `confirmation_timed_out`, `question_pending`, `target_changed`, `audit_failed`, `audit_full`, `request_id_too_long`, `capture_failed` | refused, needs_grant, needs_confirmation, declined, failed, timeout, busy, busy, failed, too_large, usage, failed | the MCP server's refusals, in `structuredContent.error` | |

**Tools:** the 26 shipped tools keep their names, arguments, results and annotations (the
parity test compares each schema byte for byte with the shipped one), and D160 adds `wm_focus`,
`wm_move` (`x`, `y`), `wm_size` (`width`, `height`), `wm_minimise`, `wm_maximise`,
`wm_restore` and `wm_close`: effect `change`, grant `run`, idempotent but for `wm_close`. Their
effects: `read` for the reading tools, `wm_capture` and `wm_menu_list`; `change` for
`clock_sync`, `app_install`, `app_open`, `edit_open` and the seven new `wm` tools; `destroy` for
`tee`, `trash`, `fetch`, `clock_set` and `app_uninstall`; `control` for `run`, the input tools
and `wm_menu_choose`. No tool is `openWorldHint` yet, as shipped.

**GUI surfaces** (D22): each command's `surface` names its GUI (Files for `ls`, the Text
Editor for `cat` and `tee`, the title bar's buttons for `wm`'s verbs, the launcher for `app`,
the Agent chip for `agentlog`), or why there is none (`ps` waits for a task manager; `wm
capture` and the input verbs are for agents, people use the window itself; `help` describes the
command line).
