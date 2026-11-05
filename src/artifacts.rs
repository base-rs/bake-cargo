// Released under the MIT License.
// Copyright, 2026, by Samuel Williams.

//! Release artifacts: project-specific files built, signed, and verified from
//! a release commit and attached to its GitHub Release.
use bake::{Context, Error, Result, Value};
use serde_json::json;
use std::collections::BTreeSet;
use std::process::Output;

use crate::version_support::Version;
use crate::workflow::{append_github_output, previous_versions, release_trailer};

/// Render the release artifacts workflow for a branch, signing environment, and the secrets its sign step receives.
pub(crate) fn release_artifacts_workflow(
    branch: &str,
    environment: &str,
    secrets: &[String],
) -> Result<String> {
    crate::github::validate_branch(branch)?;
    crate::github::validate_environment(environment)?;
    let mut seen = BTreeSet::new();
    for secret in secrets {
        validate_secret_name(secret)?;
        if !seen.insert(secret) {
            return Err(Error::new(format!("secret {secret} is listed twice")));
        }
    }

    let mut secrets_env = String::new();
    if !secrets.is_empty() {
        secrets_env.push_str("        env:\n");
        for secret in secrets {
            secrets_env.push_str(&format!(
                "          RELEASE_SECRET_{secret}: ${{{{ secrets.{secret} }}}}\n"
            ));
        }
    }

    Ok(render(
        include_str!("release-artifacts.yml"),
        &[
            ("__BRANCH__", branch),
            ("__ENVIRONMENT__", environment),
            ("__SECRETS_ENV__", &secrets_env),
        ],
    ))
}

/// Replace placeholders in one pass, so a value is never itself searched for placeholders.
fn render(template: &str, values: &[(&str, &str)]) -> String {
    let mut output = String::new();
    let mut rest = template;
    while let Some((index, key, value)) = values
        .iter()
        .filter_map(|(key, value)| rest.find(key).map(|index| (index, *key, *value)))
        .min_by_key(|(index, _, _)| *index)
    {
        output.push_str(&rest[..index]);
        output.push_str(value);
        rest = &rest[index + key.len()..];
    }
    output.push_str(rest);
    output
}

/// GitHub secret names contain letters, numbers, and underscores, do not start with a number or `GITHUB_`, and are case-insensitive; require the upper-case form so each secret has one spelling.
fn validate_secret_name(secret: &str) -> Result<()> {
    let valid = secret
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_uppercase() || byte == b'_')
        && secret
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
        && !secret.starts_with("GITHUB_");
    if valid {
        Ok(())
    } else {
        Err(Error::new(format!(
            "secret {secret:?} must be an upper-case GitHub secret name: letters, numbers, and underscores, not starting with a number or GITHUB_"
        )))
    }
}

/// Find the release commit whose artifacts to build, and check that it was released from the release branch.
///
/// Pass `--sha` with a pushed commit to use its `Release:` trailer, or `--tag` with an existing release tag. The tag must name a commit on `origin/BRANCH` whose trailer requests that tag and whose workspace version matches it. Run this from a checkout of the release branch with its full history and tags, never from the commit being checked. Writes the `release`, `tag`, `version`, and `sha` GitHub outputs.
#[bake::task(name = "cargo:release:artifacts:detect")]
pub fn detect(
    context: &mut Context,
    #[bake(default = "main")] branch: String,
    #[bake(default = "")] tag: String,
    #[bake(default = "")] sha: String,
) -> Result<Value> {
    crate::github::validate_branch(&branch)?;
    let (tag, pushed) = match (tag.is_empty(), sha.is_empty()) {
        (true, false) => {
            validate_commit_id(&sha)?;
            match release_trailer(context, &sha)? {
                Some(tag) => (tag, Some(sha)),
                None => return outputs(None),
            }
        }
        (false, true) => (tag, None),
        _ => return Err(Error::new("pass exactly one of --tag or --sha")),
    };

    let version = tag_version(context, &tag)?;
    let tagged = tagged_commit(context, &tag)?
        .ok_or_else(|| Error::new(format!("release tag {tag} does not exist")))?;
    let sha = match pushed {
        Some(sha) if sha != tagged => {
            return Err(Error::new(format!(
                "release tag {tag} names {tagged}, not the published commit {sha}"
            )));
        }
        _ => tagged,
    };
    if release_trailer(context, &sha)?.as_deref() != Some(tag.as_str()) {
        return Err(Error::new(format!(
            "commit {sha} is not the release commit for {tag}: its message must end with a 'Release: {tag}' trailer"
        )));
    }
    require_on_branch(context, &sha, &branch)?;
    let released = previous_versions(context, &sha)?;
    if released.as_deref() != Some(version.as_str()) {
        return Err(Error::new(format!(
            "the workspace version at {sha} is {}, not {version}",
            released.as_deref().unwrap_or("missing")
        )));
    }

    outputs(Some((&tag, &version, &sha)))
}

fn outputs(release: Option<(&str, &str, &str)>) -> Result<Value> {
    let (tag, version, sha) = release.unwrap_or_default();
    let flag = if release.is_some() { "true" } else { "false" };
    for (name, value) in [
        ("release", flag),
        ("tag", tag),
        ("version", version),
        ("sha", sha),
    ] {
        append_github_output(name, value)?;
    }
    Ok(json!({"release": release.is_some(), "tag": tag, "version": version, "sha": sha}))
}

/// A full commit ID, as GitHub reports it, so that the checks name exactly one commit.
fn validate_commit_id(sha: &str) -> Result<()> {
    if matches!(sha.len(), 40 | 64)
        && sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(Error::new(format!(
            "release commit {sha:?} must be a full lower-case commit ID"
        )))
    }
}

/// The version a release tag names, which also confines the tag to the configured prefix and a stable version.
fn tag_version(context: &Context, tag: &str) -> Result<String> {
    let prefix = crate::release_tag::tag_prefix(context)?;
    tag.strip_prefix(prefix.as_str())
        .filter(|version| Version::parse(version).is_ok())
        .map(str::to_owned)
        .ok_or_else(|| {
            Error::new(format!(
                "{tag:?} is not a release tag: expected {prefix:?} followed by a stable MAJOR.MINOR.PATCH version"
            ))
        })
}

fn tagged_commit(context: &Context, tag: &str) -> Result<Option<String>> {
    git(
        context,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/tags/{tag}^{{commit}}"),
        ],
    )
    .and_then(|output| match output.status.code() {
        Some(0) => Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        )),
        Some(1) => Ok(None),
        _ => Err(Error::new(format!(
            "could not read release tag {tag}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
    })
}

fn require_on_branch(context: &Context, sha: &str, branch: &str) -> Result<()> {
    git(
        context,
        &[
            "merge-base",
            "--is-ancestor",
            sha,
            &format!("refs/remotes/origin/{branch}"),
        ],
    )
    .and_then(|output| match output.status.code() {
        Some(0) => Ok(()),
        Some(1) => Err(Error::new(format!(
            "commit {sha} is not on {branch}, so it was not released from it"
        ))),
        _ => Err(Error::new(format!(
            "could not check that {sha} is on origin/{branch}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
    })
}

fn git(context: &Context, arguments: &[&str]) -> Result<Output> {
    context
        .command("git")
        .args(arguments)
        .output()
        .map_err(|error| Error::new(format!("could not run git: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Environment, Project};
    use std::fs;
    use std::process::Command;

    fn git_ok(project: &Project, arguments: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(project.root())
            .args(arguments)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn commit(project: &Project, message: &str) -> String {
        git_ok(project, &["add", "-A"]);
        git_ok(
            project,
            &["commit", "--quiet", "--allow-empty", "-m", message],
        );
        git_ok(project, &["rev-parse", "HEAD"])
    }

    /// A repository whose `origin/main` holds a tagged `v1.2.3` release commit followed by an ordinary commit.
    struct Released {
        project: Project,
        release: String,
        later: String,
    }

    fn released(environment: &mut Environment) -> Released {
        let project = Project::new();
        project.single_package("fixture", "1.2.3");
        project.cargo_proxy(environment, None);
        git_ok(&project, &["init", "--quiet"]);
        git_ok(&project, &["config", "user.name", "Test User"]);
        git_ok(&project, &["config", "user.email", "test@example.com"]);
        let release = commit(&project, "Bump patch version.\n\nRelease: v1.2.3");
        git_ok(&project, &["tag", "-a", "v1.2.3", "-m", "Release v1.2.3"]);
        let later = commit(&project, "Change something.");
        let origin = project.root().join("origin.git");
        git_ok(
            &project,
            &["init", "--bare", "--quiet", origin.to_str().unwrap()],
        );
        git_ok(
            &project,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git_ok(
            &project,
            &["push", "--quiet", "origin", "HEAD:refs/heads/main"],
        );
        git_ok(&project, &["fetch", "--quiet", "origin"]);
        environment.remove("GITHUB_OUTPUT");
        Released {
            project,
            release,
            later,
        }
    }

    fn run(project: &Project, branch: &str, tag: &str, sha: &str) -> Result<Value> {
        detect(
            &mut project.context(),
            branch.to_owned(),
            tag.to_owned(),
            sha.to_owned(),
        )
    }

    fn error(project: &Project, branch: &str, tag: &str, sha: &str) -> String {
        run(project, branch, tag, sha).unwrap_err().to_string()
    }

    #[test]
    fn renders_the_workflow_for_a_branch_environment_and_secrets() {
        let workflow = release_artifacts_workflow(
            "main",
            "android-release",
            &["KEYSTORE".to_owned(), "KEYSTORE_PASSWORD".to_owned()],
        )
        .unwrap();
        assert!(!workflow.contains("__"));
        assert!(workflow.contains("    branches: [main]\n"));
        assert!(workflow.contains("github.ref == 'refs/heads/main'"));
        assert!(workflow.contains("--branch main --tag"));
        assert!(workflow.contains("    environment: android-release\n"));
        assert!(workflow.contains(
            "        env:\n          RELEASE_SECRET_KEYSTORE: ${{ secrets.KEYSTORE }}\n          RELEASE_SECRET_KEYSTORE_PASSWORD: ${{ secrets.KEYSTORE_PASSWORD }}\n        run: |\n"
        ));
        // Only the sign step names secrets.
        assert_eq!(workflow.matches("${{ secrets.").count(), 2);
        assert!(workflow.contains("cargo bake --locked cargo:release:artifacts:detect"));
        assert!(workflow.contains("ref: ${{ needs.detect.outputs.sha }}"));
        assert!(
            workflow.contains("if: always()\n        run: rm -rf \"$RUNNER_TEMP/release-secrets\"")
        );
        assert!(workflow.contains("scripts/release-artifacts/build"));
        assert!(workflow.contains("scripts/release-artifacts/sign"));
        assert!(workflow.contains("scripts/release-artifacts/verify"));
        assert!(workflow.contains("gh release upload \"$RELEASE_TAG\" --"));
        assert!(!workflow.contains("--clobber\n"));

        let plain = release_artifacts_workflow("trunk", "release-artifacts", &[]).unwrap();
        assert!(!plain.contains("__"));
        assert!(!plain.contains("${{ secrets."));
        assert!(plain.contains("      - name: Sign the release artifacts\n        run: |\n"));
        assert!(plain.contains("    branches: [trunk]\n"));
        assert!(plain.contains("    environment: release-artifacts\n"));
    }

    #[test]
    fn rejects_invalid_workflow_settings() {
        for (branch, environment, secrets, expected) in [
            ("feature/x", "release", vec![], "branch"),
            ("main", "bad environment", vec![], "environment"),
            ("main", "", vec![], "environment"),
            ("main", "release", vec![String::new()], "secret"),
            ("main", "release", vec!["lower".to_owned()], "secret"),
            ("main", "release", vec!["9KEY".to_owned()], "secret"),
            ("main", "release", vec!["A-KEY".to_owned()], "secret"),
            ("main", "release", vec!["GITHUB_TOKEN".to_owned()], "secret"),
            (
                "main",
                "release",
                vec!["KEY".to_owned(), "KEY".to_owned()],
                "listed twice",
            ),
        ] {
            assert!(
                release_artifacts_workflow(branch, environment, &secrets)
                    .unwrap_err()
                    .to_string()
                    .contains(expected),
                "{branch} {environment} {secrets:?}"
            );
        }
        assert!(release_artifacts_workflow("main", "release", &["_KEY_2".to_owned()]).is_ok());
    }

    #[test]
    fn renders_placeholders_in_one_pass() {
        assert_eq!(
            render("a __X__ b __Y__ c", &[("__X__", "__Y__"), ("__Y__", "y")]),
            "a __Y__ b y c"
        );
    }

    #[test]
    fn detects_a_release_from_its_published_commit_or_its_tag() {
        let mut environment = Environment::new();
        let fixture = released(&mut environment);
        let output = fixture.project.root().join("github-output");
        environment.set("GITHUB_OUTPUT", output.as_os_str());
        let expected =
            json!({"release": true, "tag": "v1.2.3", "version": "1.2.3", "sha": fixture.release});

        assert_eq!(
            run(&fixture.project, "main", "", &fixture.release).unwrap(),
            expected
        );
        assert_eq!(
            fs::read_to_string(&output).unwrap(),
            format!(
                "release=true\ntag=v1.2.3\nversion=1.2.3\nsha={}\n",
                fixture.release
            )
        );
        assert_eq!(
            run(&fixture.project, "main", "v1.2.3", "").unwrap(),
            expected
        );
    }

    #[test]
    fn skips_a_published_commit_without_a_release_trailer() {
        let mut environment = Environment::new();
        let fixture = released(&mut environment);
        let output = fixture.project.root().join("github-output");
        environment.set("GITHUB_OUTPUT", output.as_os_str());

        assert_eq!(
            run(&fixture.project, "main", "", &fixture.later).unwrap(),
            json!({"release": false, "tag": "", "version": "", "sha": ""})
        );
        assert_eq!(
            fs::read_to_string(&output).unwrap(),
            "release=false\ntag=\nversion=\nsha=\n"
        );
    }

    #[test]
    fn rejects_invalid_arguments() {
        let mut environment = Environment::new();
        let fixture = released(&mut environment);
        let project = &fixture.project;

        assert!(error(project, "feature/x", "v1.2.3", "").contains("branch"));
        assert!(error(project, "main", "", "").contains("exactly one"));
        assert!(error(project, "main", "v1.2.3", &fixture.release).contains("exactly one"));
        assert!(error(project, "main", "", "abc123").contains("full lower-case commit ID"));
        assert!(
            error(project, "main", "", &fixture.release.to_uppercase())
                .contains("full lower-case commit ID")
        );
        assert!(error(project, "main", "v1.2", "").contains("not a release tag"));
        assert!(error(project, "main", "release-1.2.3", "").contains("not a release tag"));
        assert!(error(project, "main", "v1.2.4", "").contains("v1.2.4 does not exist"));
        // A full commit ID that the repository does not have.
        assert!(
            error(project, "main", "", &"1".repeat(40)).contains("could not read release commit")
        );
    }

    #[test]
    fn rejects_releases_not_published_from_the_branch() {
        let mut environment = Environment::new();
        let fixture = released(&mut environment);
        let project = &fixture.project;

        // A tag on a commit that is not a release commit.
        git_ok(project, &["tag", "v1.2.4", &fixture.later]);
        assert!(
            error(project, "main", "v1.2.4", "").contains("is not the release commit for v1.2.4")
        );

        // A release commit and tag that never reached main.
        project.single_package("fixture", "1.2.5");
        let side = commit(project, "Bump patch version.\n\nRelease: v1.2.5");
        git_ok(project, &["tag", "v1.2.5", &side]);
        assert!(error(project, "main", "v1.2.5", "").contains("is not on main"));
        assert!(error(project, "main", "", &side).contains("is not on main"));
        assert!(error(project, "missing", "v1.2.3", "").contains("could not check"));

        // A published commit whose tag names another commit.
        project.single_package("fixture", "1.2.6");
        git_ok(project, &["tag", "v1.2.6", &side]);
        let moved = commit(project, "Bump patch version.\n\nRelease: v1.2.6");
        assert!(error(project, "main", "", &moved).contains(&format!("names {side}")));

        // A published commit whose tag was never created.
        project.single_package("fixture", "1.2.7");
        let untagged = commit(project, "Bump patch version.\n\nRelease: v1.2.7");
        assert!(error(project, "main", "", &untagged).contains("v1.2.7 does not exist"));

        // A release commit on main whose workspace version is not the tag's.
        let mismatched = commit(project, "Bump patch version.\n\nRelease: v1.2.8");
        git_ok(project, &["tag", "v1.2.8", &mismatched]);
        git_ok(
            project,
            &["push", "--quiet", "origin", "HEAD:refs/heads/main"],
        );
        git_ok(project, &["fetch", "--quiet", "origin"]);
        assert!(error(project, "main", "v1.2.8", "").contains("workspace version at"));
        assert!(error(project, "main", "v1.2.8", "").contains("is 1.2.7, not 1.2.8"));
    }

    #[test]
    fn reports_git_cargo_and_output_failures() {
        let mut environment = Environment::new();
        let fixture = released(&mut environment);
        let project = &fixture.project;

        let output_directory = project.root().join("output-directory");
        fs::create_dir(&output_directory).unwrap();
        environment.set("GITHUB_OUTPUT", output_directory.as_os_str());
        assert!(run(project, "main", "v1.2.3", "").is_err());
        environment.remove("GITHUB_OUTPUT");

        project.git_proxy(&mut environment, Some("show"));
        assert!(error(project, "main", "v1.2.3", "").contains("fake git failure"));
        environment.set("BAKE_TEST_GIT_FAILURE", "");

        environment.set("CARGO", project.root().join("missing-cargo").as_os_str());
        assert!(run(project, "main", "v1.2.3", "").is_err());

        let invalid_prefix = Project::new();
        invalid_prefix.write(
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"1.2.3\"\nedition = \"2024\"\n\n[package.metadata.bake.release]\ntag-prefix = \"-bad\"\n",
        );
        assert!(error(&invalid_prefix, "main", "-bad1.2.3", "").contains("release tag prefix"));

        let not_repository = Project::new();
        not_repository.single_package("fixture", "1.2.3");
        assert!(
            error(&not_repository, "main", "v1.2.3", "").contains("could not read release tag")
        );

        environment.set("PATH", not_repository.root().as_os_str());
        assert!(error(&not_repository, "main", "v1.2.3", "").contains("could not run git"));
    }
}
