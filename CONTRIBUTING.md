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

`main-protection.json` requires: `lint`, `test`, `node-api (Node 22)`,
`node-api (Node 24)`, `coverage`, and the three `golden-cross-platform` matrix
legs.

Three CI jobs are deliberately **not** required, because they cannot gate a PR:

| Job | Why it is not required |
|---|---|
| `bench-smoke` | `if: github.event_name == 'push'` — never runs on a PR, so requiring it would block every PR forever |
| `security` | `continue-on-error` on PRs — always reports success there |
| `fidelity-smoke` | `continue-on-error` on PRs — always reports success there |

The last two follow the existing "advisory on PR, blocking on push to `main`"
policy in [ci.yml](.github/workflows/ci.yml). Now that everything reaches `main`
through a merge, their blocking tier fires *after* the merge rather than
preventing it. Removing the `continue-on-error` conditions would close that gap.
