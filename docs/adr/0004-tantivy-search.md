# ADR-0004: Tantivy as the primary search index, with SQLite as the record

## Status

Accepted.

## Date

2026-05-31

## Context

Search has to answer in milliseconds while the results list is rebuilt on every
keystroke, over an index that can hold hundreds of thousands of items.

The original implementation queried SQLite with a leading-wildcard `LIKE`, which
cannot use an index. Measured query times ranged from tens of milliseconds to
several seconds depending on the term. A full-text engine was needed, but the item
record also had to stay queryable for things a search index does not hold well:
usage counters, recency, per-query selection memory, and referential operations
such as deleting an item.

## Decision

Use two stores, with a clear owner for each.

| Store | Role |
|---|---|
| Tantivy | Primary full-text index. Holds id, title, normalised title, path, subtitle, kind, extension, use count, and last access. |
| SQLite | Canonical item record, usage counters, per-query selection memory, and a trigram full-text table as fallback |

A file query goes to Tantivy first and falls back to SQLite's trigram table. Ordinary
application queries are answered from an in-memory snapshot without touching either
store.

Both stores are owned by the indexer and written by it. The UI records launches and
selections, which is the single deliberate exception.

## Alternatives considered

### SQLite alone

Keep one store and add full-text capability through SQLite.

This is what the trigram table provides, and it is retained as the fallback.
Rejected as the primary path because its query cost on a large item set was the
original problem.

### Tantivy alone

Drop SQLite and keep everything in the search index.

Rejected. A search index is not a good record store: usage counters would need
rewrites of whole documents to update a number, and operations that are trivial in
a relational table become index maintenance. It would also remove the fallback.

### An external search service

Rejected for the same reason the launch path is in-process: it adds a service to
install, start, and keep alive for a desktop launcher.

## Consequences

- Two stores must stay consistent. Every write path has to update both, and the
  index rebuilds if either is missing or unreadable. A deleted item must be removed
  from both.
- Ranking happens in Nex, not in the engine. Tantivy returns candidates and the
  crate's own scoring tiers decide the order, so the ordering is tunable without
  changing the index.
- The SQLite trigram table is real code that must keep working, because it is the
  answer when the full-text index cannot serve a query.
- Deleting `index.sqlite3` while Nex is closed is safe: both stores are rebuilt.
- The index has a bounded size by configuration, so a very large drive is a
  deliberate trade rather than an unbounded cost.
