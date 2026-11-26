# Discovery, indexing, and search

Discovery answers "what exists", the index answers "where is it", and search
answers "which one did they mean". In Nex all three run in the indexer, and the UI
searches a snapshot the indexer publishes. The relevant modules are
`discovery`, `mft_discovery`, `index_store`, `tantivy_search`, `search`, and
`core_service`.

## Two discovery backends

File discovery has a fast path and a fallback.

| Backend | What it reads | When it is used |
|---|---|---|
| MFT | The NTFS master file table directly, plus the USN change journal for deltas | NTFS volumes whose item cap makes a directory walk impractical |
| walkdir | Directory enumeration, then `ReadDirectoryChangesW` for changes | Everything else, including non-NTFS roots |

`file_discovery_backend = "auto"` picks per root: MFT when the root's cap is large
enough to justify it, walkdir otherwise. Setting `"walkdir"` forces the fallback
everywhere.

The MFT path needs the elevated helper to read the journal, because reading it
requires privilege a Medium process does not have. When the helper is not ready,
the delta pass waits and then falls back to a full walk rather than failing. That
is why a helper outage costs a slower scan and not an empty index.

A full MFT pass publishes progress counts, which is what the small progress window
in the bottom right of the screen reports. Short incremental passes do not show it.

### What is indexed

Applications come from the Start Menu for the current user and for all users, the
Desktop, registered program records, the Windows Settings pages, and the Control
Panel applets. Files and folders come from `discovery_roots`.

`App Paths` registrations are enumerated straight from the registry (`HKCU`,
`HKLM` and the `WOW6432Node` view) and a registration whose executable is not on
disk is dropped. Start-menu app identities come from the shell's `AppsFolder`
namespace, enumerated as shell items: the App ID is the item's
`SIGDN_PARENTRELATIVEPARSING` name and the title is its `SIGDN_NORMALDISPLAY`
name, which is the same list `Get-StartApps` reports. No part of an app pass
starts PowerShell; `Get-StartApps` remains only as a fallback for the case where
the namespace enumerates nothing, and is expected never to run.

An AppsFolder entry is also checked against the filesystem before it becomes an
app row. Uninstalling a program leaves its Start-menu shortcut behind with a dead
target, and the shell keeps listing the entry, so without that check the app
stayed searchable after it was gone and clicking it handed Explorer a path that
no longer existed - which it answers by opening the Documents folder. A parsing
name of the form `{FOLDERID-...}\relative\path` or `C:\…\App.exe` is resolved
and dropped when the target is missing; a bare AppUserModelID
(`…_8wekyb3d8bbwe!App`) names a package rather than a file and is kept. The
filter runs every pass, and its schema stamp is bumped whenever the filtering
changes so a cached app list cannot keep the stale rows.

Each pass also re-checks the launch target of every stored app row and deletes the
ones whose target is gone, because the shell keeps listing an uninstalled app for
several seconds - waiting for it to let go is what made a removal take 5-10 s.
Measured here, a deleted target left the index in 0.3 s.

An application pass runs two seconds after a change is seen, then again after 1,
2, 4 and 8 seconds. The follow-ups are not padding: an installer writes over
several seconds, and the shell's own view of the Start-menu folder lags the
filesystem, so the first pass after an install can legitimately see nothing. Each
pass also asks the shell that the roots changed (`SHChangeNotify(SHCNE_UPDATEDIR)`)
before reading them. Measured here, a new shortcut reached the index in 3.5 s and a
deleted one in 2.5 s, where a single pass left the first install waiting for the
ten-minute reconcile.

An application pass is skipped while its stamp is unchanged and the last scan is
recent, so the stamp has to be able to see an install. It carries each Start-menu
root's modification time for that reason: a new shortcut changes it. The app
watcher also forgets the provider's stamp on any change it observes, so a pass
runs immediately rather than waiting for the window to elapse. Both were needed -
with only the reconcile window, installing a program could take half an hour to
appear, which reads as "needs a restart". The provider window and the watcher's
reconcile are both ten minutes; a full app pass is a few hundred milliseconds, so
agreeing with the watcher is cheap.

Executable publishers come from each file's own version resource
(`\StringFileInfo\<lang><codepage>\CompanyName`), read in process. That reader
lives in `win_version_resource.rs` rather than in `discovery.rs`, because the
updater reads the same resource tree for a different field: the installed
build's `FileVersion`. Both used to be PowerShell `FileVersionInfo` lookups,
which is the argument shape a user's antivirus flagged (CIQ-201).

### Watchers and the reconcile window

Each root has its own directory watcher and consumer. A watcher that cannot open
its root — an unmounted drive, a network path that comes up later, a directory
replaced underneath it — is not treated as a dead root: the watcher thread stays
alive, retries with backoff, and asks for a full resync the moment the root
appears. A change-journal overflow, an exhausted 64 KiB completion buffer, and a
watcher restart after a failure all ask for the same resync, because events were
missed in each case.

The 300 s reconcile pass is a safety net behind those signals rather than the
primary one: it exists for a change none of them reports, at the cost of a
delta pass that measures in the hundreds of milliseconds.

Built-in exclusions always apply to files and folders, never to applications:
`Windows`, `Program Files`, `AppData`, `node_modules`, `.git`, `__pycache__`, and
a short list of shell junctions. `discovery_exclude_roots` adds to them.

Three caps bound what the index will hold: `index_max_items_total`,
`index_max_items_per_root`, and `discovery_max_depth`. A capped discovery is a
deliberate trade: a complete index of a multi-terabyte drive would cost more than
it returns.

## The two stores

| Store | Holds | Owner |
|---|---|---|
| SQLite, `index.sqlite3` | Items, usage counters, per-query selection memory, and a trigram full-text table | Indexer writes, UI reads history |
| Tantivy, beside the database | The primary full-text index | Indexer only |

The SQLite item table is the canonical record. Tantivy is a search structure over
it, holding the id, the title and its normalised form, the path, the subtitle, the
kind, the extension, the use count, and the last-access time.

A file query goes to Tantivy first, and SQLite's trigram table is the fallback for
when the full-text index cannot answer. Ordinary application queries are answered
from the in-memory snapshot without touching either store.

The stores are rebuilt if either is missing or unreadable. Deleting
`index.sqlite3` while Nex is closed costs a rescan and nothing else.

## Seeding a keystroke

A one- or two-letter query keeps a floor of app hits in front of its seed
(`SHORT_QUERY_APP_FLOOR = 24` in `tantivy_search.rs`). One Tantivy query over
the whole index ranks by BM25, and the index holds far more file paths
containing a letter than app names starting with it, so a single letter could
return a handful of apps - or none - while files and folders listed normally.
The floor runs the same prefix query with `kind` fixed to `app` and puts those
hits first; the UI truncates the seed before ranking, so an app that is absent
or late in the seed is an app the user never sees. The seed limit is unchanged
on purpose: a larger limit admits more files and pays the keystroke cost again.

Each keystroke asks the indexer for candidates (`search_seed`) under a 500 ms
budget. If the budget expires, the answer is empty, or the call fails, the UI
ranks its own hot cache instead, which is a much smaller set. That fallback used
to be invisible.

Both sides now count it. The indexer reports `requests`, `answered`, `empty`,
`failed`, `over_budget`, `avg_ms`, `max_ms` and `budget_ms` in its `status`
payload; `over_budget` counts seeds whose own work reached the budget, which is
the honest "too slow" signal and only the indexer can measure it. The UI counts
which path each keystroke took (`asked`, `answered`, `empty`, `failed`,
`prefix_cache`, `not_ui_client`) and writes that line to the log at most every
30 s, because those counters live in the UI process where a separate `--status`
run cannot read them.

Measured on a development machine: 120 seeds against an idle index and 1,500
across a full filesystem pass, all answered, `avg_ms=2`, `max_ms=6`,
`over_budget=0`. Those queries were single words against an already-built
index — a prefix query during a Tantivy rebuild is the heavier case, and the
counters are what will show it.

## Ranking

Results are ordered by how the query matched, then by how you have used the item.

The matching tiers, strongest first:

1. **Exact** title match
2. **Prefix** match
3. **Substring** match
4. **Fuzzy** match, allowing skipped characters

Within a tier, shorter titles and earlier matches score higher, and a fuzzy match
is penalised for each gap it had to jump. Ties break on the use counter and
recency, so the launcher you open daily sits above the one you installed once.

Search also consults per-query selection memory: if you repeatedly pick the third
result for the same query, that choice is remembered and promoted.

Applications sort before files and folders, which sort before actions. That order
is fixed and does not depend on Grid view.

## Snapshot publishing

The indexer publishes its state under a generation number, and the UI holds the
last generation it applied.

- A read at the current generation returns "unchanged" and costs nothing.
- A read one generation behind returns a delta of upserts and deletions.
- Any other case returns a full snapshot.

The UI never blocks on this. A query typed during a publish is answered from the
snapshot the UI already has, and the newer one applies on the next read. A result
can therefore be a moment stale, and the first keystroke after a long idle is not
waiting on the indexer.

The UI also records launches and selections. That is the one write the UI makes to
the shared database, because the click knows what was chosen and the indexer does
not.

## Query parsing

`query_dsl` turns typed text into a structured query: a mode override, an
extension filter, time windows, include groups for `OR`, and exclude terms for
`NOT`. It is the single place operators are interpreted, so a new operator is
added there and nowhere else.

The user-facing syntax is documented in
[../user/search-syntax.md](../user/search-syntax.md).

## Quirks worth knowing

- **`kind:` is advertised and not implemented.** The comment block at the top of
  the generated `config.toml` offers `kind:` as an operator, but the parser has no
  case for it. `@files` and `mode:files` work.
- **The `everything` backend string still loads.** An old configuration saying
  `file_discovery_backend = "everything"` is accepted and treated as `"auto"`,
  because the Everything SDK backend was removed. Configurations from that era
  keep working without an edit.
- **One pass per volume, matched against every root.** MFT discovery groups the
  configured roots by volume and walks the volume once, matching each entry
  against all roots on it. Adding a second root on a volume already being scanned
  therefore does not double the scan cost.

## Next

- [architecture.md](architecture.md) for how the indexer relates to the UI
- [configuration.md](configuration.md) for the caps and their validation ranges
- [../adr/](../adr/) for why MFT and Tantivy were chosen
