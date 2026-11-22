# ADR-0008: Confirm by tool annotation, with one deliberate exception

## Status

Accepted.

## Date

2026-10-07

## Context

The assistant can act on the user's machine. It can create, edit, move, and delete
files, and it can run a shell command. Its output is a language model's, which means
the tool calls it produces are untrusted input even when the user's intent was
benign.

The failure mode is not a bad answer. It is a wrong file operation on a real
machine, performed because a confirmation dialog was clicked through.

The design question was therefore not "how do we prompt" but "which actions deserve
a prompt at all", because a prompt that appears too often trains the user to dismiss
it, and a dismissed prompt provides no protection while feeling like one.

## Decision

Classify every tool with the Model Context Protocol annotation vocabulary, and
decide confirmation with one free-standing predicate:

```
destructive                  -> confirm
read_only                    -> run
neither, but open_world only -> run
otherwise                    -> confirm
```

In practice:

| Confirmed | Unattended |
|---|---|
| `create_file`, `create_folder`, `edit_file` | `search_paths`, `read_file` |
| `move_path`, `copy_path`, `delete_path` | `open_path`, `reveal_path` |
| `run_command`, `set_scope` | `open_url`, `pc_specs` |

Two refinements sit on top:

- A URL that came from tool output rather than from the user is confirmed, because
  provenance matters more than the tool's class.
- "Approve the rest of this turn" never covers `delete_path`, `run_command`, or
  `set_scope`, and never survives into the next turn.

The predicate is kept separate from the tool enum so it can be tested against
annotations a future tool might carry.

## Alternatives considered

### Confirm every tool call

Rejected. Prompting for a filename lookup makes the prompt meaningless, and a user
who clicks through ten prompts will click through the eleventh.

### Confirm nothing and rely on the model's judgement

Rejected. The model's judgement is the thing being protected against.

### `!read_only && open_world` as the run condition

An earlier draft. It let a wholly unclassified tool run unattended, because
`!read_only && open_world` was true for a tool that declared nothing. The current
form is the intersection of what that draft and the fail-safe draft were each
trying to say.

### Confirmation as a modal that blocks the loop

Rejected because a blocked thread cannot be cancelled cleanly. The loop is
suspended with a request id instead, and a superseded or expired card cannot approve
a later action.

## Consequences

- The classification lives in code, per tool, and a new tool without annotations is
  treated as dangerous. This is intentional friction: adding a tool requires a
  deliberate decision about its class.
- The 90-second card timeout and the single pending slot mean a slow user is asked
  again rather than approved late. The alternative, approving an action the user has
  moved on from, is worse.
- Some genuinely safe actions are confirmed, because a tool that changes nothing
  still belongs to a class that changes things. The cost is a click.
- The rule must be enforced by the host, not the page. A page that receives
  untrusted content cannot be trusted to decide whether an action is allowed.
- Provenance gating for links adds state that has to travel with each tool call,
  which makes the tool call representation more than a name and its arguments.
