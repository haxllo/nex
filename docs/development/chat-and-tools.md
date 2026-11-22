# The assistant, its tools, and its trust boundary

The assistant is a tool-calling loop. A model proposes a call, Rust decides
whether that call is allowed and whether the user must approve it, Rust executes
it, and the result goes back to the model. The model never touches the filesystem
and never runs anything itself.

The relevant modules are `chat`, `chat_tools`, and `chat_eval`.

## Providers

Two provider shapes are supported, both OpenAI-compatible in transport.

| Provider | Authentication |
|---|---|
| `codex` | A ChatGPT account sign-in, with credentials stored for the Windows user |
| `openai-compatible` | A base URL and an API key |

An API key is stored through the secure storage module, encrypted against the
Windows account, and never written into `config.toml`.

## The loop

1. The UI sends a user turn.
2. The model is called with the conversation, the system prompt, and the tool
   declarations.
3. If the model returns a tool call and no approval is needed, or approval is
   granted, the tool runs and its output is appended.
4. The output is fed back and the model continues, until it produces a final
   answer or the turn is cancelled or out of attempts.

A confirmation suspends the loop rather than blocking a thread. Cancelling the
request, starting a new turn, or letting the 90-second card lapse releases the
suspension and the loop ends without running the call.

Approval is bound to a request id. A stale confirmation from a superseded turn
cannot approve the next turn's action, which is the failure that makes a
confirmation dialog worse than none.

## The confirmation predicate

One free-standing predicate decides whether a tool call needs a card. It is kept
separate from the tool enum so it can be tested against annotations a future tool
might carry.

```
destructive      -> confirm
read_only        -> run
neither, and open_world only -> run
otherwise        -> confirm
```

The reasoning behind the shape:

- A destructive tool always confirms. No exceptions, no configuration.
- A read-only tool never confirms. Prompting for a search trains the user to click
  through prompts, and a prompt that is clicked through is worse than no prompt.
- A tool that reaches the network while changing nothing locally is a browser tab.
  That is the single unconfirmed non-read-only case, and it is closed for links the
  model chose rather than links the user supplied.
- Anything unclassified is treated as dangerous. A tool that declares neither
  property describes nothing we understand, so it fails safe.

## What a tool may not do

Refusals happen before a card is shown, and the message is written so the model can
change approach rather than repeat itself.

| Refusal | Because |
|---|---|
| Writing into system locations | An assistant should not be writing under `System32` |
| A recursive scan from a drive root | It takes minutes and the output is truncated anyway |
| Deleting a drive root | It is a drive root |
| Any URL scheme other than http and https | `file:` and `javascript:` are not links to open |
| A relative path where an absolute one is required | The working directory is not part of the contract |

A refusal still counts as an attempt. A model that keeps re-asking after a refusal
is stopped rather than looped.

## Tool output is untrusted input

Tool output comes from files, and files can contain anything. A filename can carry
text shaped to steer the model.

Two defences are in place:

- Tool output is wrapped in a data boundary and labelled as data, so content is
  not mistaken for instructions.
- Links are gated by provenance. A URL that appeared inside tool output is not the
  same as a URL the user typed, and it needs confirmation before it is opened.

## Scope

`set_scope` records the folder a conversation works in, so later calls can name a
file without repeating the full path. It confirms, it changes nothing on disk, and
it never carries into a new conversation.

## Limits

| Limit | Value |
|---|---|
| Command timeout | 30 seconds, or 5 minutes when the model marks the command slow |
| Tool output | Cut at 16 KB, keeping the start and the end |
| File reads | One page per call, by offset and limit |
| Confirmation card | Lapses after 90 seconds |

A file read is pageable rather than split into a head and a tail. A head and tail
split can show a file's ends but cannot answer a question about its middle.

## Where approval state lives

The pending confirmation is a single slot, not a queue. A new request supersedes
it: the previous card can no longer be accepted, because an approval that arrives
after the user has moved on would apply to work they never saw.

## Testing this area

`chat_eval` holds the evaluation harness for prompt and tool behaviour, and
`chat_tools` carries unit tests for the confirmation predicate, the refusal
messages, and the path rules. Both are worth running alone while iterating:

```bash
cargo test -p nex --lib chat
```

## Next

- [../user/assistant.md](../user/assistant.md) for the user-facing behaviour
- [architecture.md](architecture.md) for where this sits in the process model
- [../adr/](../adr/) for the approval-boundary decision
