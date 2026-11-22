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
