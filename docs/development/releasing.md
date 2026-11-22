# Releasing

A release is published by pushing a tag. CI does the building, packaging, and
publishing; nothing is produced locally and nothing is hand-written except the
What's New page.

## What a release requires

Two changes on `master`, in one commit:

1. **The version, in all three crates.** `apps/core/Cargo.toml`,
   `apps/ui/Cargo.toml`, and `apps/indexer/Cargo.toml` each carry their own version,
   because each build script stamps its own binaries from its own package. CI fails
   the release if the tag and any of the three disagree.
2. **A rewritten `docs/whats-new.md`.** This is the in-app page, not the release
   page. Its `release:` marker must name the version being released.

Both are required. The document is compiled into the binary by
`apps/core/build.rs`, which is why it has to land before the tag and in the same
commit as the version bump.

## What is never hand-written

`docs/releases/v<ver>-notes.md` is the GitHub release page. It is drafted from the
commit log by `scripts/windows/release-notes-from-commits.ps1`, which the workflow
runs when the file is absent. Writing it by hand produces a second, competing
narrative.

The two documents are not interchangeable:

| File | Audience | Author |
|---|---|---|
| `docs/releases/v<ver>-notes.md` | people browsing GitHub releases | the workflow, from commits |
| `docs/whats-new.md` | the person using Nex | a human, every release |

`docs/whats-new.md` carries its own format and voice instructions in HTML
comments at the top of the file. Read them before editing; they are the authority
on that page's structure.

## The procedure

```bash
# 1. Bump the version in all three crates and rewrite docs/whats-new.md,
#    then commit both together.
git commit --author="..." -m "Release <ver>"

# 2. Tag only once those commits are on master.
git tag v<ver>
git push origin master --tags
```

Pushing the tag starts `.github/workflows/release.yml`.

## What the workflow does

| Step | Failure mode |
|---|---|
| Verify the tag matches all three crate versions | Hard failure: a mismatched version never publishes |
| Resolve the release notes, drafting them from the commit log if absent | Hard failure if drafting fails |
| Check `docs/whats-new.md` for the marker and for visible text | Missing file fails; stale marker only warns; empty body fails |
| Build all three binaries in release | Hard failure |
| Package the zip and manifest, then the installer | Hard failure |
| Open the GitHub release | Hard failure |

The What's New check is deliberately asymmetric. A file with no text outside its
comments is a defect and fails the release, because the in-app page would be
empty. A file whose marker names a different version only warns, because the text
may legitimately still be correct and judging that is a human call.

## Artifacts

```
artifacts/windows/
  nex-<ver>-windows-x64.zip
  nex-<ver>-windows-x64.setup.exe
  nex-<ver>-windows-x64-manifest.json
```

The zip is staged by `package-windows-artifact.ps1`, which also copies a runtime
validation checklist and the release notes template into the package. The installer
is built by `package-windows-installer.ps1` from the Inno Setup script.

Do not run either packaging script by hand to cut a release. They exist for CI and
for producing a local artifact when you specifically need one.

## Writing the What's New page

The rules live in comments at the top of `docs/whats-new.md`. The ones that get
broken most often:

- The first `## ` line is the heading. There is exactly one.
- Every `### ` becomes a collapsed drawer. Four or five is plenty.
- Write for the person using Nex. No ticket ids, no commit words, no file names,
  no test counts.
- A fix is written as a fix: the symptom it removes, in the reader's words.
- Do not put a version, a date, or the word changelog in the text. The page
  supplies all three itself.

## After the release

Pushing the tag publishes. The GitHub release page shows
`docs/releases/v<ver>-notes.md`; the app shows `docs/whats-new.md` once per
version, behind the pill that appears after an update.

## Publishing to winget (optional, manual)

winget does not follow releases. A version appears there only when someone submits
it, so the published version silently falls behind unless you act. That is how the
first manifests sat at 1.0.1 for six months while the project reached 3.3.0.

It is deliberately separate from `release.yml`. Submission needs a token that can
open pull requests on `microsoft/winget-pkgs`, and an expired token or an outage
there must not turn a successful release red. Running it per release is also
unnecessary: a version superseded while still queued for review wastes moderator
time.

```bash
# through Actions
github.com/haxllo/nex/actions/workflows/winget.yml -> Run workflow

# or from a shell
gh workflow run winget.yml -f version=3.4.0

# locally, to inspect what would be submitted
./scripts/windows/publish-winget.ps1 -Version 3.4.0 -DryRun
```

The script regenerates the manifests from the published release asset, not from
whatever is in `manifests/`. It hashes the downloaded installer and cross-checks it
against the hash recorded in that release's `manifest.json`, so a mismatch stops
before anything reaches GitHub. It needs `WINGET_TOKEN` in repository secrets, a
PAT with `public_repo` scope; the default `GITHUB_TOKEN` cannot open pull requests
in another repository.

If a pull request for this package is already open, the new version replaces the
old one on that same branch rather than opening a second one. winget validation
allows exactly one package version per pull request.

Expect a first submission to take a week or more: `New-Package` cannot auto-merge
and needs a moderator. Later versions go through automated validation and merge
within hours.

## Next

- [testing.md](testing.md) for the gates that run on every pull request
- [../README.md](../README.md) for how the two release documents relate
- `scripts/windows/` for the packaging scripts themselves
