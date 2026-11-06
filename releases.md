# Releases

## v0.8.0

- Add `cargo:setup:artifacts`, which generates a `release-artifacts.yml` workflow that builds, signs, verifies, and attaches project-specific files to the GitHub Release from the released commit, and `cargo:release:artifacts:detect`, which checks that the commit is on the release branch and matches the tag and version. Projects supply only `scripts/release-artifacts/build`, `sign`, and `verify`; signing secrets reach only the sign step, on a runner that never ran the build.
- Block updates to release tags, as well as deletion, in the release tag ruleset that `cargo:setup:github:apply` manages, so nobody can move a published tag to another commit.

## v0.7.0

- Add `cargo:release:patch`, `cargo:release:minor`, `cargo:release:major`, and `cargo:release:bump --version X.Y.Z`, which from a clean checkout of the release branch create a `release-{tag}` branch, bump the version, commit with the `Release: {tag}` trailer, validate the candidate, push, and open the release pull request with the release notes and trailer as its body.
- Remove the `cargo:version:patch`, `cargo:version:minor`, `cargo:version:major`, and `cargo:version:bump` tasks. The `cargo:release:*` tasks perform the same version update and run the `cargo:after_version_bump` hook, so a release is one command followed by review and merge.
- Set the repository's squash merge commit title and message to the pull request title and body in `cargo:setup:github:apply`, and show the setting in `cargo:setup:github:plan`, so the release trailer survives a squash merge with review fix-ups.

## v0.6.1

- Fail `cargo:setup:github:apply` before changing anything when a reviewer team cannot access the repository, printing the command that grants access, and list such teams in `cargo:setup:github:plan`. After saving the environment, read it back and fail when GitHub has silently dropped a requested reviewer.

## v0.6.0

- Require no pull request approvals by default in `cargo:setup:github`, so maintainers merge their own pull requests once checks pass. Pass `--approvals N` to require independent review.
- Restrict a publishing environment that deploys from all branches to the protected release branch, keeping existing branch policies.
- Require approval before running workflows for first-time contributors' fork pull requests, keeping a stricter existing policy.

## v0.5.4

- Depend on `base-markdown` under the `base-rs` organization, replacing `core-markdown`.
- Require migrated Bake License and Bake Releases dependencies.

## v0.5.3

- Support configurable release tag prefixes, including matching release notes, trailers, GitHub Releases, and tag protections. Existing projects continue to use `v` by default.

## v0.5.2

- Migrate to the source-backed `core-markdown` document AST, preserving supported Markdown behavior and source bytes.

## v0.5.1

- Use `core-markdown` in place of `socketry-markdown`.
- Require `bake-license` and `bake-releases` versions that also use `core-markdown`.

## v0.5.0

- Require an explicit `Release: vVERSION` Git trailer to request publication; version changes alone no longer trigger a release. Regenerate the publishing workflow to validate PR-head trailers against the merged candidate, and preserve the trailer when squash merging.
- Allow renamed packages to begin their own version history.
- Install the Bake launcher from the `bake` crate in generated publishing workflows.

## v0.4.3

- Align the readme title, release instructions, and related project links.

## v0.4.2

- Adopt `socketry-project` 0.3.7 for shared project tasks and Markdown normalization.
- Require the aggregate test and coverage result for pull request merges.
- Require publishing checks and test results by default when configuring repository protections.
- Update Bake usage examples and agent context guidance.

## v0.4.1

- Treat a missing base Cargo manifest as the initial package release.

## v0.4.0

- Remove the temporary `releases:github:release` compatibility alias in favor of `cargo:releases:github:release`.
- Replace the inline publish workflow release scripts with standard Bake Cargo tasks for detection, validation, publishing, tagging, and GitHub Release sync.

## v0.3.0

- Remove the redundant `bake_cargo::cargo` module. Task functions are available from the crate root and semantic modules; registered `cargo:*` names remain unchanged.

## v0.2.10

- Declare compatibility with the Bake 0.x API so task libraries can share one task registry when upgrading to crate-derived task namespaces.

## v0.2.9

- Expose `cargo:releases:github:release` as the canonical GitHub release task and retain `releases:github:release` as a temporary compatibility alias. Both names work with existing Bake versions and crate-derived namespaces.

## v0.2.8

- Move the shared Rust release process to `socketry-project` and remove the duplicate publishing context from Bake Cargo.
- Preserve repository-admin bypass for pull request merges when applying branch rulesets.

## v0.2.7

- Use the pull request base commit when detecting release changes.

## v0.2.6

- Validate release headings from parsed Markdown in both Bake and the publishing workflow.

## v0.2.5

- Document local path dependencies for private same-repository Bake packages.

## v0.2.4

- Create and synchronize GitHub Releases from `releases.md` after crates.io publication.

## v0.2.3

- Switch the runtime dependency from `socketry-bake` to `bake` 0.17.0.

## v0.2.2

- Correct the bootstrap task name in publishing guidance.

## v0.2.1

- Add Bake Agent Context tasks to the project's development executable.
- Link the shared Rust context guidance from the README.
- Allow the workflow initiator to approve the crates.io deployment.
- Use the generated `check` job as the default required branch check.

## v0.2.0

- Add an optional `cargo:after_version_bump` project hook for version-specific automation.

## v0.1.0

- Rename the Cargo task library to `bake-cargo` and give its tasks a Cargo namespace.
- Let project-local hooks compose license and release-note updates after Cargo version changes.
- Keep `bake-releases` focused on release-document tasks.
- Publish Cargo workspaces after reviewed release changes merge to the configured branch.
- Require environment reviewers before publishing and create version tags after successful uploads.
- Turn `cargo:release` into a release-candidate check instead of creating tags.
- Generate workflows that discover workspace packages dynamically.
- Move the Cargo, GitHub, and crates.io release tasks into their own repository.
- Support Cargo workspace discovery, publishing workflow setup, and trusted publishing.
- Add shared workspace version bumps and a tag-based release task.
- Skip package versions already published when running generated release workflows.
