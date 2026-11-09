// Released under the MIT License.
// Copyright, 2026, by Samuel Williams.

use bake::{Context, Error, Result, Value};
use base_markdown::{
    Document, ParseOptions,
    stream::{EventKind, Tag},
};
use serde_json::json;
use std::fs;

use crate::version_support::Component;

/// Bump the patch version, commit it with the release trailer, validate it, and open the release pull request.
#[bake::task(name = "cargo:release:patch")]
pub fn patch(context: &mut Context, #[bake(default = "main")] branch: String) -> Result<Value> {
    start(context, Bump::Component(Component::Patch), &branch)
}

/// Bump the minor version, commit it with the release trailer, validate it, and open the release pull request.
#[bake::task(name = "cargo:release:minor")]
pub fn minor(context: &mut Context, #[bake(default = "main")] branch: String) -> Result<Value> {
    start(context, Bump::Component(Component::Minor), &branch)
}

/// Bump the major version, commit it with the release trailer, validate it, and open the release pull request.
#[bake::task(name = "cargo:release:major")]
pub fn major(context: &mut Context, #[bake(default = "main")] branch: String) -> Result<Value> {
    start(context, Bump::Component(Component::Major), &branch)
}

/// Set an explicit version, commit it with the release trailer, validate it, and open the release pull request. The current version is accepted for a first release that is not yet published or tagged.
#[bake::task(name = "cargo:release:bump")]
pub fn bump(
    context: &mut Context,
    #[bake(
        named,
        help = "New stable workspace version in MAJOR.MINOR.PATCH form."
    )]
    version: String,
    #[bake(default = "main")] branch: String,
) -> Result<Value> {
    start(context, Bump::Version(version), &branch)
}

enum Bump {
    Component(Component),
    Version(String),
}

impl Bump {
    fn apply(&self, context: &mut Context) -> Result<Value> {
        match self {
            Self::Component(Component::Patch) => crate::version::patch(context),
            Self::Component(Component::Minor) => crate::version::minor(context),
            Self::Component(Component::Major) => crate::version::major(context),
            Self::Version(version) => crate::version::bump(context, version.clone()),
        }
    }

    fn subject(&self) -> String {
        match self {
            Self::Component(Component::Patch) => "Bump patch version.".to_owned(),
            Self::Component(Component::Minor) => "Bump minor version.".to_owned(),
            Self::Component(Component::Major) => "Bump major version.".to_owned(),
            Self::Version(version) => format!("Bump version to {version}."),
        }
    }
}

/// Prepare a release from a clean checkout of `base`: a `release-{tag}` branch
/// with one commit carrying the `Release: {tag}` trailer, validated by
/// `prepare`, pushed, and opened as a pull request whose body ends with the
/// same trailer so a squash merge configured to use the body keeps it.
fn start(context: &mut Context, bump: Bump, base: &str) -> Result<Value> {
    crate::github::validate_branch(base)?;
    let status = run(
        context,
        "git",
        &["status", "--porcelain", "--untracked-files=no"],
    )?;
    if !status.is_empty() {
        return Err(Error::new(
            "the working tree has uncommitted changes; commit or stash them before starting a release",
        ));
    }
    let current = run(context, "git", &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if current != base {
        return Err(Error::new(format!(
            "releases start from the {base} branch, but {current} is checked out"
        )));
    }

    let bumped = bump.apply(context)?;
    open_pull_request(context, &bump, base, &bumped)
}

/// Commit the bumped version, validate it, push, and open the pull request.
fn open_pull_request(context: &Context, bump: &Bump, base: &str, bumped: &Value) -> Result<Value> {
    let version = new_version(bumped)?;
    let tag = crate::release_tag(context, &version)?;
    let branch = format!("release-{tag}");
    // A first release of the version already in the manifests changes no version, so it is not described as a bump.
    let subject = if bumped.get("previous_version") == bumped.get("version") {
        format!("Release version {version}.")
    } else {
        bump.subject()
    };
    let trailer = format!("Release: {tag}");
    run(context, "git", &["switch", "--create", &branch])?;
    run(
        context,
        "git",
        &[
            "commit",
            "--all",
            "--message",
            &subject,
            "--message",
            &trailer,
        ],
    )?;

    let prepared = package(context, &version).map_err(|error| {
        Error::new(format!(
            "{error}; the release commit remains on the {branch} branch for inspection"
        ))
    })?;

    let body = format!("{}\n\n{trailer}\n", prepared.notes.trim());
    run(
        context,
        "git",
        &["push", "--set-upstream", "origin", &branch],
    )?;
    let pull_request = run(
        context,
        "gh",
        &[
            "pr", "create", "--base", base, "--head", &branch, "--title", &subject, "--body", &body,
        ],
    )?;

    Ok(json!({
        "version": version,
        "tag": tag,
        "branch": branch,
        "packages": prepared.packages,
        "pull_request": pull_request,
        "next": format!("Review and merge the pull request, keeping the {trailer} trailer on the merged commit, then approve the publishing environment deployment."),
    }))
}

fn new_version(result: &Value) -> Result<String> {
    result
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::new("version task did not return the new workspace version"))
}

fn run(context: &Context, program: &str, arguments: &[&str]) -> Result<String> {
    let output = context
        .command(program)
        .args(arguments)
        .output()
        .map_err(|error| Error::new(format!("could not start `{program}`: {error}")))?;
    if !output.status.success() {
        return Err(Error::new(format!(
            "{program} {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub(crate) fn prepare(context: &Context, version: &str) -> Result<Value> {
    let prepared = package(context, version)?;

    Ok(json!({
        "version": version,
        "tag": prepared.tag,
        "packages": prepared.packages,
        "status": "ready for a release pull request",
        "next": format!("Commit the reviewed release changes with a Release: {} Git trailer, then open a pull request. Preserve that trailer on the final merged commit to request publishing after the crates-io environment approval.", prepared.tag),
    }))
}

struct Prepared {
    tag: String,
    packages: Vec<String>,
    notes: String,
}

fn package(context: &Context, version: &str) -> Result<Prepared> {
    let tag = crate::release_tag(context, version)?;
    let packages = crate::cargo_support::workspace_packages(context)?;
    if packages.is_empty() {
        return Err(Error::new("the workspace has no publishable packages"));
    }

    let release_notes_path = context.root().join("releases.md");
    let release_notes = fs::read_to_string(&release_notes_path).map_err(|error| {
        Error::new(format!(
            "could not read {}: {error}",
            release_notes_path.display()
        ))
    })?;
    if !contains_release_heading(&release_notes, &tag) {
        let heading = format!("## {tag}");
        return Err(Error::new(format!(
            "{} must contain a {heading:?} release heading",
            release_notes_path.display()
        )));
    }
    let notes = bake_releases::extract_notes(&release_notes, &tag)
        .map_err(|error| {
            Error::new(format!(
                "could not extract release notes from {}: {error}",
                release_notes_path.display()
            ))
        })?
        .to_owned();

    let mut package_arguments = vec!["package".to_owned(), "--locked".to_owned()];
    for package in &packages {
        package_arguments.extend(["--package".to_owned(), package.name.clone()]);
    }
    crate::cargo_support::run_cargo_arguments(context, &package_arguments)?;

    Ok(Prepared {
        tag,
        packages: packages.into_iter().map(|package| package.name).collect(),
        notes,
    })
}

pub(crate) fn contains_release_heading(release_notes: &str, tag: &str) -> bool {
    let document = Document::parse(release_notes, &ParseOptions::default())
        .expect("the built-in Markdown parser produces a valid document");
    document.children(document.root()).any(|id| {
        if !matches!(
            document.node(id).unwrap().kind,
            EventKind::Start(Tag::Heading(2))
        ) {
            return false;
        }
        document
            .text_content(id)
            .strip_prefix(tag)
            .is_some_and(|remainder| {
                remainder.is_empty() || remainder.chars().next().is_some_and(char::is_whitespace)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::{contains_release_heading, prepare};

    #[test]
    fn requires_an_exact_release_heading() {
        assert!(contains_release_heading(
            "## Unreleased\n\n## v1.2.3\n",
            "v1.2.3"
        ));
        assert!(contains_release_heading(
            "## v1.2.3 - Improvements\n",
            "v1.2.3"
        ));
        assert!(!contains_release_heading("## v1.2.30\n", "v1.2.3"));
        assert!(!contains_release_heading("## v1.2.3-rc.1\n", "v1.2.3"));
        assert!(!contains_release_heading(
            "```markdown\n## v1.2.3\n```\n",
            "v1.2.3"
        ));
        assert!(!contains_release_heading("> ## v1.2.3\n", "v1.2.3"));
        assert!(!contains_release_heading(
            "A paragraph before a heading.\n",
            "v1.2.3"
        ));
    }

    #[test]
    fn resolves_heading_markup_and_ignores_nested_headings() {
        assert!(contains_release_heading("## **v1.2.3**\n", "v1.2.3"));
        assert!(contains_release_heading("## v1.2.&#51;\n", "v1.2.3"));
        assert!(!contains_release_heading("- ## v1.2.3\n", "v1.2.3"));
    }

    #[test]
    fn prepares_release_archives_after_validating_workspace_and_release_notes() {
        use crate::test_support::{Environment, Project};

        let mut environment = Environment::new();
        let project = Project::new();
        project.single_package("fixture", "1.2.3");
        project.write("releases.md", "# Releases\n\n## v1.2.3\n\nRelease notes.\n");
        project.cargo_proxy(&mut environment, None);

        let result = super::prepare(&project.context(), "1.2.3").unwrap();
        assert_eq!(result["status"], "ready for a release pull request");
        assert_eq!(result["packages"][0], "fixture");
        assert!(
            project
                .cargo_arguments()
                .contains("package --locked --package fixture")
        );
    }

    #[test]
    fn rejects_ambiguous_release_notes_before_packaging() {
        use crate::test_support::{Environment, Project};

        let mut environment = Environment::new();
        let project = Project::new();
        project.single_package("fixture", "1.2.3");
        project.write(
            "releases.md",
            "## v1.2.3\n\nFirst entry.\n\n## v1.2.3\n\nDuplicate entry.\n",
        );
        project.cargo_proxy(&mut environment, None);

        assert!(
            prepare(&project.context(), "1.2.3")
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
        assert!(!project.cargo_arguments().contains("package --locked"));
    }

    #[test]
    fn rejects_empty_workspaces_missing_notes_and_failed_packaging() {
        use crate::test_support::{Environment, Project};

        let mut environment = Environment::new();
        let failed_metadata = Project::new();
        failed_metadata.single_package("fixture", "1.2.3");
        let cargo = failed_metadata.executable(
            "cargo-failed",
            "#!/bin/sh\necho metadata denied >&2; exit 1\n",
        );
        environment.set("CARGO", cargo.as_os_str());
        assert!(prepare(&failed_metadata.context(), "1.2.3").is_err());

        let empty = Project::new();
        empty.write(
            "Cargo.toml",
            "[workspace]\nmembers = [\"private\"]\nresolver = \"3\"\n",
        );
        empty.write("private/Cargo.toml", "[package]\nname = \"private\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n");
        empty.write("private/src/lib.rs", "// private\n");
        empty.cargo_proxy(&mut environment, None);
        assert!(
            prepare(&empty.context(), "0.1.0")
                .unwrap_err()
                .to_string()
                .contains("no publishable packages")
        );

        let missing_notes = Project::new();
        missing_notes.single_package("fixture", "1.2.3");
        missing_notes.cargo_proxy(&mut environment, None);
        assert!(
            prepare(&missing_notes.context(), "1.2.3")
                .unwrap_err()
                .to_string()
                .contains("could not read")
        );

        missing_notes.write("releases.md", "# Releases\n\n## v1.2.2\n\nOld.\n");
        assert!(
            prepare(&missing_notes.context(), "1.2.3")
                .unwrap_err()
                .to_string()
                .contains("must contain a")
        );

        let failed_package = Project::new();
        failed_package.single_package("fixture", "1.2.3");
        failed_package.write("releases.md", "## v1.2.3\n\nNotes.\n");
        failed_package.cargo_proxy(&mut environment, Some("package"));
        assert!(prepare(&failed_package.context(), "1.2.3").is_err());
    }

    mod start {
        use super::super::{Bump, new_version, run};
        use crate::test_support::{Environment, Project};
        use crate::version_support::Component;
        use bake::{Context, Registry, Result};
        use serde_json::json;
        use std::process::Command;

        #[bake::task(name = "cargo:after_version_bump")]
        fn rename_unreleased(context: &mut Context, version: String) -> Result<()> {
            let path = context.root().join("releases.md");
            let document = std::fs::read_to_string(&path).unwrap();
            let renamed = document.replace("## Unreleased", &format!("## v{version}"));
            std::fs::write(&path, renamed).unwrap();
            Ok(())
        }

        fn git(root: &std::path::Path, arguments: &[&str]) -> String {
            let output = Command::new("git")
                .current_dir(root)
                .args(arguments)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {arguments:?} failed");
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        }

        struct Fixture {
            project: Project,
            origin: Project,
        }

        impl Fixture {
            /// A committed single-package checkout of `main` with an empty bare origin,
            /// a logging `gh`, and proxies for `cargo` and `git`.
            fn new(environment: &mut Environment) -> Self {
                for name in ["GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"] {
                    environment.set(name, "Fixture");
                }
                for name in ["GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL"] {
                    environment.set(name, "fixture@example.com");
                }
                environment.set("GIT_CONFIG_GLOBAL", "/dev/null");
                environment.set("GIT_CONFIG_NOSYSTEM", "1");

                let project = Project::new();
                project.single_package("fixture", "1.2.3");
                project.write(
                    "releases.md",
                    "# Releases\n\n## Unreleased\n\n- A change.\n\n## v1.2.3\n\n- Earlier.\n",
                );
                project.cargo_proxy(environment, None);
                project.executable(
                    "gh",
                    "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$BAKE_TEST_GH_LOG\"\nif [ -n \"$BAKE_TEST_GH_FAILURE\" ]; then echo 'fake gh failure' >&2; exit 1; fi\nprintf 'https://github.com/socketry/fixture/pull/7\\n'\n",
                );
                environment.set(
                    "BAKE_TEST_GH_LOG",
                    project.root().join("gh-arguments.log").as_os_str(),
                );
                environment.set("BAKE_TEST_GH_FAILURE", "");
                project.git_proxy(environment, None);

                let origin = Project::new();
                git(origin.root(), &["init", "--quiet", "--bare"]);
                git(
                    project.root(),
                    &["init", "--quiet", "--initial-branch=main"],
                );
                git(
                    project.root(),
                    &["remote", "add", "origin", origin.root().to_str().unwrap()],
                );
                git(project.root(), &["add", "."]);
                git(
                    project.root(),
                    &["commit", "--quiet", "--message", "Initial commit."],
                );
                Self { project, origin }
            }

            fn context(&self) -> Context {
                let mut registry = Registry::new();
                registry.register(rename_unreleased_task()).unwrap();
                registry.context(self.project.root())
            }

            fn gh_arguments(&self) -> String {
                std::fs::read_to_string(self.project.root().join("gh-arguments.log"))
                    .unwrap_or_default()
            }
        }

        #[test]
        fn opens_a_release_pull_request_for_each_kind_of_bump() {
            let mut environment = Environment::new();
            type Task = fn(&mut Context) -> Result<serde_json::Value>;
            let cases: [(&str, Task, &str, &str); 4] = [
                (
                    "patch",
                    |context| super::super::patch(context, "main".to_owned()),
                    "1.2.4",
                    "Bump patch version.",
                ),
                (
                    "minor",
                    |context| super::super::minor(context, "main".to_owned()),
                    "1.3.0",
                    "Bump minor version.",
                ),
                (
                    "major",
                    |context| super::super::major(context, "main".to_owned()),
                    "2.0.0",
                    "Bump major version.",
                ),
                (
                    "bump",
                    |context| super::super::bump(context, "3.1.0".to_owned(), "main".to_owned()),
                    "3.1.0",
                    "Bump version to 3.1.0.",
                ),
            ];
            for (name, task, version, subject) in cases {
                let fixture = Fixture::new(&mut environment);
                let result = task(&mut fixture.context()).unwrap();
                let tag = format!("v{version}");
                let branch = format!("release-{tag}");
                assert_eq!(result["version"], version, "{name}");
                assert_eq!(result["tag"], tag, "{name}");
                assert_eq!(result["branch"], branch, "{name}");
                assert_eq!(result["packages"], json!(["fixture"]), "{name}");
                assert_eq!(
                    result["pull_request"], "https://github.com/socketry/fixture/pull/7",
                    "{name}"
                );

                let root = fixture.project.root();
                assert_eq!(git(root, &["rev-parse", "--abbrev-ref", "HEAD"]), branch);
                assert_eq!(git(root, &["log", "-1", "--format=%s"]), subject);
                assert_eq!(
                    git(
                        root,
                        &["log", "-1", "--format=%(trailers:key=Release,valueonly)"]
                    ),
                    tag
                );
                assert_eq!(
                    git(root, &["status", "--porcelain", "--untracked-files=no"]),
                    ""
                );
                assert!(
                    std::fs::read_to_string(root.join("releases.md"))
                        .unwrap()
                        .contains(&format!("## {tag}\n"))
                );
                assert_eq!(
                    git(fixture.origin.root(), &["rev-parse", &branch]),
                    git(root, &["rev-parse", "HEAD"])
                );
                assert!(
                    fixture
                        .project
                        .cargo_arguments()
                        .contains("package --locked --package fixture")
                );

                let gh_arguments = fixture.gh_arguments();
                assert!(
                    gh_arguments.starts_with("pr\ncreate\n--base\nmain\n--head\n"),
                    "{gh_arguments}"
                );
                assert!(
                    gh_arguments.contains(&format!(
                        "--title\n{subject}\n--body\n- A change.\n\nRelease: {tag}\n"
                    )),
                    "{gh_arguments}"
                );
            }
        }

        #[test]
        fn refuses_to_start_from_a_dirty_tree_or_another_branch() {
            let mut environment = Environment::new();
            let fixture = Fixture::new(&mut environment);
            let root = fixture.project.root();

            assert!(
                super::super::patch(&mut fixture.context(), "release/1".to_owned())
                    .unwrap_err()
                    .to_string()
                    .contains("branch must contain")
            );

            fixture.project.write("src/lib.rs", "// changed\n");
            assert!(
                super::super::patch(&mut fixture.context(), "main".to_owned())
                    .unwrap_err()
                    .to_string()
                    .contains("uncommitted changes")
            );
            git(root, &["checkout", "--quiet", "--", "src/lib.rs"]);

            git(root, &["switch", "--quiet", "--create", "feature"]);
            let error = super::super::patch(&mut fixture.context(), "main".to_owned()).unwrap_err();
            assert!(
                error.to_string().contains("feature is checked out"),
                "{error}"
            );
            assert_eq!(fixture.gh_arguments(), "");
        }

        #[test]
        fn commits_refreshed_nested_workspace_lockfiles_with_the_release() {
            let mut environment = Environment::new();
            let fixture = Fixture::new(&mut environment);
            let root = fixture.project.root();
            fixture.project.write(
                "benchmarks/Cargo.toml",
                "[package]\nname = \"benchmarks\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[workspace]\n\n[dependencies]\nfixture = { path = \"..\" }\n",
            );
            fixture
                .project
                .write("benchmarks/src/lib.rs", "// fixture\n");
            // Cargo sets `CARGO` for the tests it runs.
            assert!(
                Command::new(environment.original("CARGO").unwrap())
                    .current_dir(root.join("benchmarks"))
                    .args(["generate-lockfile", "--offline"])
                    .status()
                    .unwrap()
                    .success()
            );
            git(root, &["add", "benchmarks"]);
            git(root, &["commit", "--quiet", "--message", "Add benchmarks."]);

            super::super::minor(&mut fixture.context(), "main".to_owned()).unwrap();

            assert_eq!(
                git(root, &["show", "--format=", "--name-only", "HEAD"]),
                "Cargo.toml\nbenchmarks/Cargo.lock\nreleases.md"
            );
            assert!(
                std::fs::read_to_string(root.join("benchmarks/Cargo.lock"))
                    .unwrap()
                    .contains("name = \"fixture\"\nversion = \"1.3.0\"\n")
            );
            assert_eq!(
                git(root, &["status", "--porcelain", "--untracked-files=no"]),
                ""
            );
        }

        #[test]
        fn releases_the_current_version_when_it_is_not_yet_published_or_tagged() {
            let mut environment = Environment::new();
            let fixture = Fixture::new(&mut environment);
            let root = fixture.project.root();
            // A new crate: its first version is already in the manifest and has no release notes heading yet.
            std::fs::write(
                root.join("releases.md"),
                "# Releases\n\n## Unreleased\n\n- First release.\n",
            )
            .unwrap();
            git(
                root,
                &[
                    "commit",
                    "--quiet",
                    "--all",
                    "--message",
                    "Prepare the first release.",
                ],
            );
            let (api, server) =
                crate::crates_io::tests::mock_server(vec![(404, "crate not found")]);
            environment.set("BAKE_TEST_CRATES_IO_API", &api);

            let result = super::super::bump(
                &mut fixture.context(),
                "1.2.3".to_owned(),
                "main".to_owned(),
            )
            .unwrap();
            server.join().unwrap();
            assert_eq!(
                (result["version"].as_str(), result["tag"].as_str()),
                (Some("1.2.3"), Some("v1.2.3"))
            );
            assert_eq!(
                git(root, &["log", "-1", "--format=%s"]),
                "Release version 1.2.3."
            );
            assert_eq!(
                git(
                    root,
                    &["log", "-1", "--format=%(trailers:key=Release,valueonly)"]
                ),
                "v1.2.3"
            );
            assert!(
                std::fs::read_to_string(root.join("releases.md"))
                    .unwrap()
                    .contains("## v1.2.3\n\n- First release.")
            );
            assert!(
                fixture
                    .gh_arguments()
                    .contains("--title\nRelease version 1.2.3.\n")
            );
        }

        #[test]
        fn refuses_the_current_version_once_it_is_tagged_or_published() {
            let mut environment = Environment::new();
            let fixture = Fixture::new(&mut environment);
            git(fixture.project.root(), &["tag", "v1.2.3"]);
            let error = super::super::bump(
                &mut fixture.context(),
                "1.2.3".to_owned(),
                "main".to_owned(),
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("already tagged as v1.2.3"),
                "{error}"
            );

            let fixture = Fixture::new(&mut environment);
            let (api, server) = crate::crates_io::tests::mock_server(vec![(
                200,
                r#"{"versions":[{"num":"1.2.3"}]}"#,
            )]);
            environment.set("BAKE_TEST_CRATES_IO_API", &api);
            let error = super::super::bump(
                &mut fixture.context(),
                "1.2.3".to_owned(),
                "main".to_owned(),
            )
            .unwrap_err();
            server.join().unwrap();
            assert!(
                error
                    .to_string()
                    .contains("fixture 1.2.3 is already published"),
                "{error}"
            );
            assert_eq!(
                git(
                    fixture.project.root(),
                    &["rev-parse", "--abbrev-ref", "HEAD"]
                ),
                "main"
            );
        }

        #[test]
        fn leaves_the_release_branch_when_a_later_stage_fails() {
            let mut environment = Environment::new();

            let fixture = Fixture::new(&mut environment);
            assert!(
                super::super::bump(
                    &mut fixture.context(),
                    "1.0.0".to_owned(),
                    "main".to_owned()
                )
                .unwrap_err()
                .to_string()
                .contains("must be greater")
            );
            assert_eq!(
                git(
                    fixture.project.root(),
                    &["rev-parse", "--abbrev-ref", "HEAD"]
                ),
                "main"
            );

            let fixture = Fixture::new(&mut environment);
            git(fixture.project.root(), &["branch", "release-v1.2.4"]);
            assert!(
                super::super::patch(&mut fixture.context(), "main".to_owned())
                    .unwrap_err()
                    .to_string()
                    .contains("git switch")
            );

            let fixture = Fixture::new(&mut environment);
            environment.set("BAKE_TEST_GIT_FAILURE", "commit");
            assert!(
                super::super::patch(&mut fixture.context(), "main".to_owned())
                    .unwrap_err()
                    .to_string()
                    .contains("git commit")
            );

            let fixture = Fixture::new(&mut environment);
            environment.set("BAKE_TEST_CARGO_FAILURE", "package");
            let error = super::super::patch(&mut fixture.context(), "main".to_owned()).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("remains on the release-v1.2.4 branch"),
                "{error}"
            );
            assert_eq!(
                git(
                    fixture.project.root(),
                    &["rev-parse", "--abbrev-ref", "HEAD"]
                ),
                "release-v1.2.4"
            );
            assert_eq!(fixture.gh_arguments(), "");

            let fixture = Fixture::new(&mut environment);
            environment.set("BAKE_TEST_GIT_FAILURE", "push");
            assert!(
                super::super::patch(&mut fixture.context(), "main".to_owned())
                    .unwrap_err()
                    .to_string()
                    .contains("git push")
            );
            assert_eq!(fixture.gh_arguments(), "");

            let fixture = Fixture::new(&mut environment);
            environment.set("BAKE_TEST_GH_FAILURE", "1");
            assert!(
                super::super::patch(&mut fixture.context(), "main".to_owned())
                    .unwrap_err()
                    .to_string()
                    .contains("fake gh failure")
            );
            assert_eq!(
                git(fixture.origin.root(), &["rev-parse", "release-v1.2.4"]).len(),
                40
            );
        }

        #[test]
        fn reports_git_failures_before_changing_anything() {
            let mut environment = Environment::new();
            let fixture = Fixture::new(&mut environment);
            for stage in ["status", "rev-parse"] {
                environment.set("BAKE_TEST_GIT_FAILURE", stage);
                let error =
                    super::super::patch(&mut fixture.context(), "main".to_owned()).unwrap_err();
                assert!(error.to_string().contains("fake git failure"), "{error}");
            }
            environment.set("BAKE_TEST_GIT_FAILURE", "");
            assert_eq!(
                git(fixture.project.root(), &["log", "--format=%s"]),
                "Initial commit."
            );

            let bump = Bump::Component(Component::Patch);
            assert!(
                super::super::open_pull_request(&fixture.context(), &bump, "main", &json!({}))
                    .unwrap_err()
                    .to_string()
                    .contains("did not return")
            );
            fixture.project.write(
                "Cargo.toml",
                "[package]\nname = \"fixture\"\nversion = \"1.2.4\"\nedition = \"2024\"\n\n[package.metadata.bake.release]\ntag-prefix = \"-bad\"\n",
            );
            assert!(
                super::super::open_pull_request(
                    &fixture.context(),
                    &bump,
                    "main",
                    &json!({"version": "1.2.4"})
                )
                .unwrap_err()
                .to_string()
                .contains("prefix")
            );
        }

        #[test]
        fn reports_missing_programs_and_version_results() {
            let project = Project::new();
            assert!(
                run(
                    &project.context(),
                    "bake-cargo-missing-program",
                    &["--version"]
                )
                .unwrap_err()
                .to_string()
                .contains("could not start")
            );
            assert!(new_version(&json!({})).is_err());
            assert_eq!(
                Bump::Component(Component::Major).subject(),
                "Bump major version."
            );
        }
    }
}
