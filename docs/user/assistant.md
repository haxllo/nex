# The assistant

Press `Tab` from the search box and ask for something in plain words. The
assistant works on your PC: it reads and writes files, moves and deletes them,
and runs PowerShell commands. It is not a chat window bolted onto a launcher, and
it does not act without being told to.

## Connecting a model

The assistant needs a model. Two routes are supported, both set in Settings:

| Route | What you need |
|---|---|
| ChatGPT account | Sign in. Nex stores the credentials for your Windows account. |
| OpenAI-compatible endpoint | A base URL and an API key. Any provider that speaks the OpenAI chat format works. |

An API key is encrypted and tied to your Windows user account. It is not written
to `config.toml` and is not readable by another user on the machine.

Conversations are kept per machine in the panel's local storage. They are not
synced and not uploaded anywhere by Nex.

## What it can do

Fourteen tools are available to the model. What you see is a result; what it
actually calls is one of these.

| Tool | Does |
|---|---|
| `search_paths` | Finds a file or folder by name using the index, without walking the disk |
| `read_file` | Reads a page of a text file |
| `open_path` | Opens a file or folder |
| `reveal_path` | Shows a file in Explorer |
| `open_url` | Opens a link in your browser |
| `create_folder` | Makes a folder |
| `create_file` | Writes a new file |
| `edit_file` | Changes an existing file in place |
| `move_path` | Moves a file or folder |
| `copy_path` | Copies a file or folder |
| `delete_path` | Removes a file or folder to the Recycle Bin |
| `run_command` | Runs one PowerShell command as you, without elevation |
| `pc_specs` | Reports CPU, GPU, and memory |
| `set_scope` | Sets the folder this conversation works in |

Deletion goes to the Recycle Bin, so a mistaken delete can be undone from
Explorer.

## Approval cards

Anything that changes your machine asks first. The card names the tool and its
target in words, and it shows the exact command line for `run_command`.

| Confirmed every time | Runs without asking |
|---|---|
| `create_file`, `create_folder`, `edit_file` | `search_paths`, `read_file` |
| `move_path`, `copy_path`, `delete_path` | `open_path`, `reveal_path` |
| `run_command`, `set_scope` | `open_url`, `pc_specs` |

Two rules sit behind that table:

- A tool that modifies anything confirms, without exception.
- `open_url` is the one exception, because reaching the network while changing
  nothing locally is a browser tab rather than a file operation. A link that did
  not come from you is still confirmed, since an address the model chose is not
  the same thing as one you asked for.

You can approve a single action or the rest of the current turn. The wider
approval is limited to that turn: the next request starts again with a card.

Deletion, command execution, and setting the working folder are never covered by
"rest of turn" and ask every time. A card that is left unanswered lapses after
90 seconds and the action does not run.

## What it refuses

Some things are blocked before the card is even shown.

| Refused | Reason |
|---|---|
| Writing into system locations such as `C:\Windows\System32` | A system path is not somewhere an assistant should be writing |
| A recursive scan from a drive root | It takes minutes and the output is cut off anyway |
| Deleting a drive root | No |
| A `file:`, `javascript:`, or similar URL | Only real web addresses open |

The refusals are worded so the model can try a different approach rather than
repeating the same call. If you ask for something that lands on a refusal, ask
for the folder you actually mean instead of the drive.

## Limits

- Commands stop after 30 seconds unless the model marks the command as slow, in
  which case they get five minutes.
- Tool output is cut at 16 KB, keeping the start and the end.
- A file read returns one page at a time, which is why a long file can be read
  across several steps.
- The assistant runs as you, without elevation. It cannot do anything you could
  not do in your own terminal.

## What it will not do

Nex does not bring its own chat history to a web search, does not run skills, and
does not remember facts about you between conversations. None of those are
implemented, and the assistant does not pretend otherwise.

## Next

- [using-nex.md](using-nex.md) for the keyboard
- [settings.md](settings.md) for the model and key settings
- [../development/chat-and-tools.md](../development/chat-and-tools.md) for how the
  tool loop and its trust boundary are built
