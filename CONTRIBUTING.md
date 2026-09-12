# Contributing

Participation in this project is governed by the
[Code of Conduct](CODE_OF_CONDUCT.md).

## Branch model

`main` is the only long-lived branch. It is protected: no direct pushes, no
force-pushes, no deletion. Every change lands through a pull request with green
CI. Releases are annotated `vX.Y.Z` tags on `main` — the tag, not a branch, is
what selects a version for crates.io, PyPI, npm, and GitHub Releases.

There is no `develop`, `preprod`, or `production` branch. Those model a
continuously-deployed service where a branch's HEAD *is* an environment.
Tokenfold publishes versioned artifacts to package registries, so the release
selector is the tag and an extra long-lived branch would gate nothing.

```text
main  ─────●─────●─────●─────●─────  protected, always releasable
           ↑     ↑           ↑
        feat/  fix/       tag v0.4.1 → release.yml → publish-packages.yml
```

Work branches are short-lived and named `feat/…`, `fix/…`, `chore/…`,
`docs/…`, or `release/…`. Delete them after merge.

## Making a change

```sh
git switch main && git pull
git switch -c feat/short-description
# ... commit ...
git push -u origin feat/short-description
gh pr create --fill
gh pr merge --squash --delete-branch    # once CI is green
```

Run the checks in [AGENTS.md](AGENTS.md) before opening the PR; CI runs the
same ones.

## Cutting a release

Release commits go through a PR like everything else.

```sh
git switch main && git pull
git switch -c release/v0.4.1
# bump: Cargo.toml [workspace.package], inter-crate dep pins, cargo update -w,
#       packages/tokenfold/package.json + optionalDependencies,
#       npm install --package-lock-only, test version assertions, CHANGELOG.md
git push -u origin release/v0.4.1
gh pr create --fill && gh pr merge --squash --delete-branch

git switch main && git pull
git tag -a v0.4.1 -m "v0.4.1" && git push origin v0.4.1   # fires release.yml
# wait for the GitHub Release to exist, then:
gh workflow run publish-packages.yml -f version=0.4.1

# After the workflow succeeds, verify every registry artifact (replace VERSION).
VERSION=0.4.1
for crate in tokenfold-core tokenfold-output tokenfold-learn tokenfold-cli; do
  path="${crate:0:2}/${crate:2:2}/$crate"
  curl -sS "https://index.crates.io/$path" | jq -e --arg v "$VERSION" 'select(.vers == $v)' >/dev/null
done
curl -sS https://pypi.org/pypi/tokenfold/json | jq -e --arg v "$VERSION" '.releases[$v] | length > 0' >/dev/null
for package in tokenfold @tokenfold/cli-darwin-x64 @tokenfold/cli-darwin-arm64 @tokenfold/cli-linux-x64 @tokenfold/cli-linux-arm64 @tokenfold/cli-win32-x64; do
  encoded=${package/\//%2f}
  test "$(curl -sS "https://registry.npmjs.org/$encoded" | jq -r '.["dist-tags"].latest')" = "$VERSION"
done
```

Use the sparse crates.io index above rather than its JSON API: the JSON API
requires a policy-compliant `User-Agent` and may return HTTP 200 with an error
body to a bare `curl`, which can look like a missing release.

Release tags are protected against deletion and force-moves: a published tag is
immutable, because `publish-packages.yml` builds registry artifacts from the
release it produced.

## Repository rulesets

The protection above is stored as code in [.github/rulesets/](.github/rulesets/)
so it can be reviewed and re-applied. GitHub has no mechanism to sync these
automatically — apply them once, and re-apply after editing:

```sh
gh api --method POST repos/snchimata/tokenfold/rulesets \
  --input .github/rulesets/main-protection.json
gh api --method POST repos/snchimata/tokenfold/rulesets \
  --input .github/rulesets/release-tags.json

gh api repos/snchimata/tokenfold/rulesets --jq '.[] | "\(.id)\t\(.name)\t\(.enforcement)"'
# to update an existing one: --method PUT .../rulesets/<id>
```

Note: repository admins do **not** automatically bypass rulesets the way they
could opt out of classic branch protection. If you need an escape hatch, add a
`bypass_actors` entry to `main-protection.json` rather than deleting the rule.

## Required status checks

`main-protection.json` requires: `lint`, `test`, `security`, `eval-harness`,
`node-api (Node 22)`, `node-api (Node 24)`, `coverage`, and the three
`golden-cross-platform` matrix legs.

`security` used to be advisory on PR (its old `continue-on-error` condition),
but that gap is closed: the job is now blocking on pull requests as well, so it
is listed above as a required check.

Two CI jobs are deliberately **not** required, because they cannot gate a PR:

| Job | Why it is not required |
|---|---|
| `bench-smoke` | `if: github.event_name == 'push'` — never runs on a PR, so requiring it would block every PR forever |
| `fidelity-smoke` | `continue-on-error` on PRs — always reports success there |

`fidelity-smoke` follows the "advisory on PR, blocking on push to `main`"
policy in [ci.yml](.github/workflows/ci.yml). Now that everything reaches `main`
through a merge, its blocking tier fires *after* the merge rather than
preventing it.

**Review policy:** `main-protection.json` sets `required_approving_review_count`
to `0`. This is a deliberate choice, not an oversight: this is a
single-maintainer project, so an automatic approving review would block every PR
(a second reviewer is not available). The merge gate is instead the required
status checks above plus a deliberate maintainer review of the PR. Do not raise
the approval count expecting an automatic second reviewer — there is no one to
fill that role. If a real second reviewer becomes available, raise the count in
`main-protection.json` and re-apply the ruleset.


## Evaluation and human review

The required `eval-harness` job runs the current CLI contract, deterministic
structural-evidence gate, audit-validator tests and human-audit check. It stays
red until a real reviewer resolves stale fixture sign-off; do not bypass it by
refreshing hashes or inventing reviewer metadata. Audit metadata requires a named
reviewer, UTC ISO-8601 timestamp, full reviewed commit and exact generator command.
The check validates provenance syntax and current fixture bytes, not identity or
truth of the human attestation. In addition to parsing, the recorded reviewed
commit must be a 40-hex hash that resolves in the repository (`git cat-file`),
the timestamp must be a valid past UTC ISO-8601 instant, and the reviewer name
must not be a placeholder such as `Developer`. Generate a separate pending file
with `python eval/audit_quality_sample.py --output pending-audit.md`; overwriting an
existing audit requires explicit `--force`. Only `--check` belongs in CI.

Local ruleset changes are not deployed automatically. After the workflow is on a
PR and producing the exact job names, a maintainer must apply the ruleset using
the commands above and confirm with
`gh api repos/snchimata/tokenfold/rules/branches/main` that `security` and
`eval-harness` are required. Do not require a job name that the PR does not emit.

## Maintenance cadence and intentional limitations

The maintainer reviews dependency advisories and available updates monthly and
before each release; security advisories are triaged when reported. Update the
smallest affected set, review lockfile/license changes, and run the relevant CI
checks. Grouped update automation is deferred until it reduces maintenance work.

`ponytail:` is this repository's intentional-limitation marker. Search with
`git grep -n 'ponytail:'`; each marker should state the limit and the condition
for revisiting it. New actionable defects use `TODO:` rather than disguising
unfinished safety work as a deliberate simplification.
