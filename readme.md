# `bake-cargo`

`bake-cargo` provides reusable tasks for Cargo projects. Cargo operations register beneath `cargo`, including GitHub release creation at `cargo:releases:github:release`. Add the package to an unpublished `bake/` task binary and regenerate its task links:

```sh
cargo bake --regenerate
cargo add --manifest-path bake/Cargo.toml bake-cargo
cargo bake --regenerate
```

This crate links `bake-releases` and `bake-license`, so release-document tasks and `license:update` are also available without extra imports. The task executable links `base-project`, which provides the standard project tasks and hooks. Run `cargo bake agent:context:install` to install context from dependencies such as `bake`; generated context and skills are excluded locally through Git's `info/exclude` file and do not add rules to `.gitignore`. Shared Rust guidance lives in [Bake Agent Context](https://github.com/base-rs/bake-agent-context/blob/main/context/rust.md). For the standard Base Rust release process, use the [Releasing skill](https://github.com/base-rs/base-project/blob/main/context/releasing.md). This Readme and each task's `--help` output document the Bake Cargo commands. Link Bake Agent Context in your private `bake/` package, directly or through `base-project`, then run `cargo bake --regenerate` to link its tasks and `cargo bake agent:context:install` to install guidance from dependencies.

## Inspect and package

```sh
cargo bake cargo:packages
cargo bake cargo:package bake
```

Workspace discovery uses `cargo metadata --no-deps` and ignores packages marked `publish = false` or configured for no registry. Package names and versions are read from Cargo metadata.

## Versioning and releases

A release starts with one task from a clean checkout of the release branch (`--branch`, default `main`):

```bash
cargo bake cargo:release:patch
```

`cargo:release:minor` and `cargo:release:major` increment the other components, and `cargo:release:bump --version X.Y.Z` sets an explicit higher version. The task creates a `release-{tag}` branch, updates the shared stable version of all publishable packages in the workspace (preserving TOML formatting, updating version requirements for in-workspace dependencies, and refreshing `Cargo.lock`), and calls the project task named `cargo:after_version_bump` with the new version when one is registered. It then commits the result with the `Release: {tag}` trailer, validates and packages the committed candidate with `cargo:release`, pushes the branch, and opens a pull request with `gh` whose body is the release notes followed by the same trailer. The task refuses to start from a tree with uncommitted tracked changes or from another branch, and when validation fails after the commit it leaves the release branch in place for inspection. It never tags or publishes; review and merge the pull request, then approve the publishing environment deployment.

A project uses the `cargo:after_version_bump` hook to run `license:update`, update `releases.md`, or perform other project-specific work:

```rust,ignore
#[bake::task(name = "cargo:after_version_bump")]
fn after_version_bump(context: &mut bake::Context, version: String) -> bake::Result<()> {
    context.call("license:update", &[])?;
    let heading = bake_cargo::release_tag(context, &version)?;
    context.call("releases:update", &[&heading])?;
    Ok(())
}
```

Projects using `base-project` get a standard hook that updates the license, release notes, and generated Readme sections. Review the release commit before merging the pull request; `releases.md` must contain a heading for the new tag, which the hook creates from the `## Unreleased` section.

The release commit carries exactly one Git trailer naming the intended tag:

```text
Bump patch version.

Release: v0.5.0
```

By default, the tag is `v` followed by the exact Cargo workspace version. Set `tag-prefix` in `[workspace.metadata.bake.release]` (or `[package.metadata.bake.release]` for a single package) to use a different prefix. For example, `tag-prefix = "base-v"` produces `base-v0.1.0`. Prefixes may contain ASCII letters, numbers, hyphens, and underscores, and must not start with a hyphen; an empty prefix is supported. The release-notes heading, the trailer, and the pull request body all use the same full tag.

The trailer must end the final merged commit message; a missing trailer means no release, even when the version changed. `cargo:setup:github:apply` configures the repository to build squash commits from the pull request title and body, so a release pull request merges correctly from the GitHub UI or `gh pr merge --squash` regardless of review fix-up commits. Without that setting, GitHub concatenates the commit messages and a fix-up buries the trailer; pass a body file containing the trailer to `gh pr merge --squash --body-file` instead. Rebase merging preserves the individual commit messages, so the last commit must carry the trailer. The publish workflow runs `cargo:release` on every candidate to validate and package the committed tree.

## GitHub workflow and repository settings

Generate `.github/workflows/publish.yml` after reviewing the existing workflow:

```sh
cargo bake cargo:setup:workflow
```

It refuses to replace a different existing file. Use `--force true` only after reviewing the generated output.

The standard workflows have separate responsibilities. `test.yml` runs the project's tests and coverage checks on pull requests and pushes. `publish.yml` runs release validation, formatting, and Clippy on pull requests, but skips its ordinary test command because `test.yml` already tests the proposed changes. On pushes to `main`, `publish.yml` runs ordinary workspace tests in its `check` job before the `publish` job can start. `test.yml` also runs on pushes, so the branch gets both coverage results and the test run required before publishing. The `publish` job only runs after a successful check when the pushed tip commit requests a release.

`cargo:release:detect --base BASE --sha SHA` reads the `Release: TAG` Git trailer from exactly the specified commit. Git parses the trailer block; subject text, prose examples, and ancestor commits do not request a release. Empty, duplicate, or mismatched release trailers fail validation. The checked-out workspace must have one shared version matching the tag and a matching release heading. The base comparison rejects a decreasing version for unchanged package names at the same manifest paths; a renamed package starts its own version history, and a version prepared in an earlier commit can be released without another bump.

On pull requests the workflow reads the trailer from the PR head while validating the checked-out merge candidate. On pushes it reads the actual branch tip. Existing tags must point to the release commit, so rerunning that commit remains supported. The `cargo:release` task packages the candidate on release changes. Ordinary commits do not publish. After merge, `cargo:publish:pending` checks which workspace packages still need that version; the workflow exchanges a GitHub OIDC token for a short-lived crates.io token only when uploads remain. `cargo:release:publish` publishes the remaining packages, creates and pushes the annotated release tag, and creates or updates the matching GitHub Release through `cargo:releases:github:release`. The workflow uses the GitHub environment `crates-io` and workflow file `publish.yml` by default.

Plan repository protections before applying them. Store the desired reviewers in the project's Cargo metadata so the setup task can apply them:

```toml
[workspace.metadata.bake.release]
reviewers = ["base-rs/maintainers"]
```

For a single-package project without a workspace, use `[package.metadata.bake.release]` instead. List GitHub user logins or organization/team slugs; the setup task resolves them to GitHub IDs using the authenticated `gh` CLI. Cargo ignores this tool-specific metadata, and `cargo metadata` exposes it to Bake. An explicit `--reviewers` argument overrides the manifest value.

By default the branch ruleset requires pull requests with no approvals, the required status checks, and no force pushes. Maintainers, meaning anyone with write access, merge their own pull requests once checks pass. External contributors cannot merge, so a maintainer's merge is their sign-off. The ruleset also permits repository admins to bypass its requirements for pull request merges, as an emergency exception, while still blocking direct pushes. Pass `--approvals N` when a project needs independent review, for example when it has many maintainers who do not decide changes together. Environment reviewer bypass is configured separately. Review the plan before applying it:

```sh
cargo bake cargo:setup:github:plan \
  --branch main \
  --wait-timer 0
cargo bake cargo:setup:github:apply \
  --branch main \
  --wait-timer 0
```

GitHub silently drops environment reviewers who cannot access the repository: the update succeeds, but nobody can approve releases. Give each reviewer team access to the repository before applying, for example with the maintain role:

```sh
gh api --method PUT orgs/base-rs/teams/maintainers/repos/base-rs/PROJECT -f permission=maintain
```

The plan lists reviewer teams without access under `reviewers.missing_repository_access`, with the command that grants it. The apply task checks each team named as `organization/team` before changing anything, and fails with that command when a team lacks access; it does not grant access itself, since that is a permission change for a maintainer to make deliberately. After saving the environment, the apply task reads it back and fails, naming the missing reviewers and how to fix them, when GitHub has dropped any requested reviewer, including users who are not collaborators and teams given as `Team:ID`.

If `--repository owner/name` is omitted, the GitHub repository is inferred from the `origin` Git remote. `--checks` is repeatable and should match status check names already reported by GitHub. When `--wait-timer` is omitted, the existing environment wait timer is left unchanged. At least one reviewer is required, and the selected list replaces the environment's current reviewer list. Both tasks use the authenticated `gh` CLI to resolve reviewer names. The apply task also needs permission to manage repository rulesets and environments. It creates or updates the two rulesets managed by this package and configures the named environment's wait timer, reviewer list, and deployment branch policy. An environment that deploys from all branches is restricted to the `--branch` branch protected by the managed ruleset, so only merged changes can publish; an existing branch policy is kept. The task uses a named branch policy rather than GitHub's protected-branches option, which does not recognize branches protected only by rulesets. The apply task also requires maintainer approval before running workflows for first-time contributors' fork pull requests, keeping a stricter existing policy that requires approval for all external contributors, and sets the repository's squash merge commit title and message to the pull request title and body so release trailers survive squash merges. Existing managed rulesets are replaced with the reviewed desired settings; unrelated rulesets are left alone. The release tag ruleset blocks deleting or moving a release tag, with no bypass, so a published tag always names the commit that was released. It does not restrict creating tags, because the publish workflow pushes the tag with its `GITHUB_TOKEN`; `cargo:release:publish` refuses a tag that already exists at a different commit. A mistaken tag can only be removed by an administrator disabling the ruleset deliberately. When `--checks` is omitted, the ruleset requires the publishing workflow's `check` job and the testing workflow's stable `test-result` aggregate job. Configure at least one required reviewer on the `crates-io` environment to require approval before publishing. The setup allows the workflow initiator to approve their own deployment, so a maintainer releases with one approval click rather than a bypass. GitHub administrators can bypass environment rules by default. To make approval mandatory for administrators too, disable that bypass in the environment's GitHub settings. Tag pushes do not trigger publication.

## Trusted publishing

After a package has been published once, crates.io allows its owners to register a GitHub Actions trusted publisher. Preview and apply the configuration with:

```sh
cargo bake cargo:trusted-publishing:plan bake
export CARGO_REGISTRY_TOKEN=...
cargo bake cargo:trusted-publishing:configure bake
```

The token must have the crates.io **Trusted Publishing** endpoint scope. For `bootstrap`, it also needs permission to publish the selected crate because the task passes the same `CARGO_REGISTRY_TOKEN` to Cargo and the crates.io API. The request uses Cargo's raw token value in the `Authorization` header. It is read from the environment, never placed in command arguments or printed. The desired configuration is inferred from `origin` and includes `publish.yml` and `crates-io` by default. Re-running configuration is idempotent.

For a crate that has not yet been published, `cargo:bootstrap PACKAGE` performs the initial `cargo publish --locked --package PACKAGE`, then registers the trusted publisher. This is an explicit publication task: review package contents and release state first. If the upload succeeds but publisher setup fails, the error identifies the follow-up configure task. It does not enable trusted-publishing-only mode automatically. Once the GitHub workflow succeeds, you can enable that registry requirement:

```sh
cargo bake cargo:trusted-publishing:require bake --required true
```

Keep that setting disabled until the configured workflow has successfully published. The registry setting is reversible with `--required false`.

## Scope

Create a GitHub Release from the notes under its matching heading after the release tag exists:

```sh
cargo bake cargo:releases:github:release vX.Y.Z --draft true
```

The task uses the authenticated `gh` CLI. Omit `--draft true` to publish the release immediately.

The Cargo release integration supports crates.io and GitHub Actions. The `cargo:release:*` tasks edit Cargo package version fields and local dependency requirements, commit, push, and open a pull request, but never tag or publish. The GitHub release task publishes a GitHub Release explicitly. The setup task only manages its named repository rulesets, and GitHub may require repository or organization plan features for some settings.

## Releasing

Prepare a release with `cargo bake cargo:release:patch` (or `minor`, `major`, or `bump --version X.Y.Z`) from a clean checkout of `main`; it bumps the version, commits with the `Release: vVERSION` trailer, validates the candidate, and opens the pull request. After review and merge, GitHub Actions publishes the release when the configured `crates-io` environment approves it, then creates or updates the matching GitHub Release from `releases.md`. Follow the shared [Releasing skill](https://github.com/base-rs/base-project/blob/main/context/releasing.md) for the standard process.

## Releases

<!-- bake-readme:releases:start -->

See [releases.md](releases.md) for the full release history.

### v0.7.0

- Add `cargo:release:patch`, `cargo:release:minor`, `cargo:release:major`, and `cargo:release:bump --version X.Y.Z`, which from a clean checkout of the release branch create a `release-{tag}` branch, bump the version, commit with the `Release: {tag}` trailer, validate the candidate, push, and open the release pull request with the release notes and trailer as its body.
- Remove the `cargo:version:patch`, `cargo:version:minor`, `cargo:version:major`, and `cargo:version:bump` tasks. The `cargo:release:*` tasks perform the same version update and run the `cargo:after_version_bump` hook, so a release is one command followed by review and merge.
- Set the repository's squash merge commit title and message to the pull request title and body in `cargo:setup:github:apply`, and show the setting in `cargo:setup:github:plan`, so the release trailer survives a squash merge with review fix-ups.

### v0.6.1

- Fail `cargo:setup:github:apply` before changing anything when a reviewer team cannot access the repository, printing the command that grants access, and list such teams in `cargo:setup:github:plan`. After saving the environment, read it back and fail when GitHub has silently dropped a requested reviewer.

### v0.6.0

- Require no pull request approvals by default in `cargo:setup:github`, so maintainers merge their own pull requests once checks pass. Pass `--approvals N` to require independent review.
- Restrict a publishing environment that deploys from all branches to the protected release branch, keeping existing branch policies.
- Require approval before running workflows for first-time contributors' fork pull requests, keeping a stricter existing policy.

<!-- bake-readme:releases:end -->

## See Also

- [`bake`](https://github.com/base-rs/bake).

- [`bake-releases-cargo`](https://github.com/base-rs/bake-releases-cargo).

## Contributing

Please open an issue or pull request on [GitHub](https://github.com/base-rs/bake-cargo).

### Agent Context

Run `cargo bake agent:context:install` to install shared context and skills. Read `.agents/context/index.md` to find relevant guides, follow `agents.md` if present, and apply skills under `.agents/skills/`. The installer preserves repository-owned `agents.md`; it does not create or regenerate that file.
