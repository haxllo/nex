<!-- release: 2.41.0 -->

## Chat works in your files

Nex can read a file, write one, make a folder, move something, or run a command from the same chat. Every step that touches your disk arrives as a card you can refuse.

A conversation can also carry a folder, so "open last week's invoices" finds them without a path.

### Chat

- A large file is read in order, so the middle is no longer a guess.
- Writing, editing and deleting all work, and a delete goes to the Recycle Bin. Something executable is flagged before it is saved.
- Commands run through PowerShell and print as they go. Stop stops the command, not just the answer.
- When the model runs out of steps there is a Continue button.
- A failed answer gets its own block, so a dropped connection leaves the reply above it alone.

### The folder a conversation is working in

- The folder sits in the bar above your message. Clicking it lists what is there and opens anything in it.
- **+** attaches a file from that folder to your next message, up to three.
- **×** clears it, and the model is told, so it stops treating bare names as paths.
- A step the model asks for mid-task can set the folder for the rest of that task.

### Approvals

- The card names what it is about to touch first: the file, the folder, the command.
- **Allow the rest** covers the remaining steps of that task. Deleting, running commands and opening unfamiliar links still ask every time.
- A step that never ran says why.

### Search and the panel

- Tab opens Chat from the search box, and the box says so.
- Closing a view keeps the panel at its full height.
- A long answer has a control that jumps back to the newest message.
- Compact view shows no hover hints.

### Media

- Each playing app keeps its own session, so switching players leaves the right track on screen and the right dot lit.
- Album art follows the track, with a spinning record when a track has none.

### Fixes

- Log lines start with a local date and time.
- Quitting does not freeze when the indexer is slow to answer.
- The hotkey works over a fullscreen game, and the tray says when it is being held back.
- Uninstalling an app from the menu returns to the panel instead of hanging it.
- Removing a saved API key is acknowledged, and a provider saved without a URL is refused.
