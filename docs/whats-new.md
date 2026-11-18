<!-- release: 2.41.0 -->

## Chat works in your files

Nex can read a file, write one, make a folder, move something, or run a command from the same chat, and every step that touches your disk arrives as a card you can refuse.

A conversation can also carry a folder. Point one at your invoices and "open last week's" finds them without a path.

Elsewhere the panel got quieter. You can see what a step is doing while it runs, and jump to the newest message in a long answer.

### Chat

- **Reading is paged.** A large file is read in order with the marker below it, instead of both ends and a guess at the middle.
- **Writing, editing and deleting** are all there, and a delete goes to the Recycle Bin.
- **Commands run through PowerShell**, printed as they go rather than all at the end, and Stop stops the command too.
- When the model runs out of steps it offers Continue instead of asking you to retype the request.
- A failed answer has its own block, so one network drop no longer wipes the reply above it.

### The folder a conversation is working in

- The folder sits in the bar above your message. Clicking it lists what is there and opens anything in it.
- **+** attaches a file from that folder to your next message, up to three.
- **×** clears it, and the model is told, so it stops treating bare names as paths.
- A step the model asks for mid-task can set the folder for the rest of that task.

### Approvals

- The card names the target first: the file, the folder, the command.
- **Allow the rest** covers the remaining steps of that task. Deleting, running commands and opening unfamiliar links still ask every time.
- A step that never ran says why rather than going quiet.

### Search and the panel

- Tab now opens Chat from the search box, and the placeholder says so.
- Closing a view no longer collapses the panel to a sliver.
- Compact view drops the hover hints it has no room to show.
- This page is a banner, a heading and collapsed sections, so a release reads as a changelog.

### Media

- Each playing app keeps its own session, so switching players no longer scrambles the track that is shown.
- Album art is fetched per track, with a spinning record when a track has none.

### Fixes

- Log lines start with a local date and time.
- Quitting no longer freezes when the indexer is slow to answer.
- The hotkey works over a fullscreen game, and the tray says when it is suppressed.
- Uninstalling an app from the menu no longer deadlocks the panel.
- Removing a saved API key is acknowledged, and a provider saved without a URL is refused.
