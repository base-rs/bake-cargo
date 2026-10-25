// Released under the MIT License.
// Copyright, 2026, by Samuel Williams.

use bake::{Context, Error, Result, Value};
use core_markdown::{
    Document, ParseOptions,
    stream::{EventKind, Tag},
};
use serde_json::json;
use std::fs;

pub(crate) fn prepare(context: &Context, version: &str) -> Result<Value> {
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
    bake_releases::extract_notes(&release_notes, &tag).map_err(|error| {
        Error::new(format!(
            "could not extract release notes from {}: {error}",
            release_notes_path.display()
        ))
    })?;

    let mut package_arguments = vec!["package".to_owned(), "--locked".to_owned()];
    for package in &packages {
        package_arguments.extend(["--package".to_owned(), package.name.clone()]);
    }
    crate::cargo_support::run_cargo_arguments(context, &package_arguments)?;

    Ok(json!({
        "version": version,
        "tag": tag,
        "packages": packages.iter().map(|package| package.name.as_str()).collect::<Vec<_>>(),
        "status": "ready for a release pull request",
        "next": format!("Commit the reviewed release changes with a Release: {tag} Git trailer, then open a pull request. Preserve that trailer on the final merged commit to request publishing after the crates-io environment approval."),
    }))
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
}
