# Search syntax

A plain query matches everything by name. The operators below narrow it. Quoted
phrases are kept together, so `"my report"` is one term rather than two.

All of this is on by default. Setting `search_dsl_enabled = false` in
`config.toml` turns the operators off and treats the whole query as plain text.

## Modes

A mode restricts results to one kind.

| Prefix | Restricts to |
|---|---|
| `@all` | Everything |
| `@apps`, `@app` | Applications |
| `@files`, `@file` | Files |
| `@folders`, `@folder` | Folders |
| `@clipboard`, `@clip` | Clipboard history |
| `@actions`, `@action` | Actions and web searches |

The same thing can be written as a filter, which composes with other terms:

| Filter | Value |
|---|---|
| `mode:` | `all`, `apps`, `files`, `folders`, `actions`, `clipboard` |

`mode:files invoice` and `@files invoice` mean the same thing.

A bare `@clip` or `@actions` with nothing after it switches the box into command
mode rather than filtering, because those two modes are lists you pick from.

## Command mode

`>` at the start of the box enters command mode, which searches actions and web
searches instead of files. Everything you type after it becomes the argument:

| Type | Result |
|---|---|
| `> uninstall firefox` | An uninstall action for a matching application |
| `> github.com` | Opens the site in your default browser |
| `> how to grep` | A web search row using your configured provider |

`@` works as a shortcut for the same thing when followed by a word that is not a
mode name. `@clip` and `@actions` also enter command mode.

## Filters

### By extension

`ext:md` and `extension:md` restrict to one extension. A leading dot is accepted,
so `ext:.md` is the same query.

```
ext:pdf invoice
```

### By time

`modified:` and `created:` restrict by when the file was last changed or first
created. Three windows are recognised.

| Value | Window |
|---|---|
| `today` | Since midnight |
| `week`, `last_week` | The last seven days |
| `month`, `last_month` | The last thirty days |

```
modified:today report
created:week screenshot ext:png
```

## Combining terms

| Operator | Effect |
|---|---|
| space | All terms must match |
| `AND` | Same as a space, and may be written out for clarity |
| `OR` | Either side may match |
| `NOT` | The next term must not match |
| `-term` | Shorthand for `NOT term` |

```
invoice OR receipt
report -draft
ext:md NOT archived
```

`OR` groups what came before it against what follows, so `alpha beta OR gamma`
means `(alpha AND beta) OR gamma`. Put an explicit term on both sides rather than
relying on precedence when the intent matters.

## Creation and URL affordances

When a query matches nothing locally, Nex offers to make the thing instead. These
rows appear only when there is no real match, so they never displace a result.

| Query looks like | Row offered |
|---|---|
| Ends with `\` | Create a folder with that name |
| Ends with a known extension | Create a file with that name |
| A bare domain such as `github.com` | Open the site in your browser |

Where the file or folder is created is set by `default_create_dir`, which falls
back to your Desktop. The extensions offered are `create_file_extensions`. The
domains recognised as URLs are matched against `url_tlds`.

These are controlled by `create_actions_enabled` and
`open_url_in_default_browser`.

## What is not supported

`kind:file` and `kind:folder` appear in the comment block at the top of the
generated `config.toml`, and in older README text, but the parser does not
implement them. Use `@files` or `@folders`, or the equivalent `mode:` filter.

## Next

- [using-nex.md](using-nex.md) for the keyboard
- [settings.md](settings.md) for `search_dsl_enabled` and the related keys
