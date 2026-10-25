// Released under the MIT License.
// Copyright, 2026, by Samuel Williams.

use bake::{Context, Error, Result};
use toml_edit::DocumentMut;

/// Return the release tag for a version, using `bake.release.tag-prefix` from workspace or package metadata.
/// The default prefix is `v`; the same tag identifies the release notes and GitHub Release.
pub fn release_tag(context: &Context, version: &str) -> Result<String> {
    crate::version_support::Version::parse(version)?;
    Ok(format!("{}{version}", tag_prefix(context)?))
}

pub(crate) fn tag_prefix(context: &Context) -> Result<String> {
    let path = context.root().join("Cargo.toml");
    let manifest = std::fs::read_to_string(&path)?;
    let document = manifest
        .parse::<DocumentMut>()
        .map_err(|error| Error::new(format!("could not parse {}: {error}", path.display())))?;
    let configured = ["workspace", "package"].into_iter().find_map(|section| {
        document
            .get(section)?
            .get("metadata")?
            .get("bake")?
            .get("release")?
            .get("tag-prefix")
    });
    let prefix = match configured {
        Some(value) => value
            .as_str()
            .ok_or_else(|| Error::new("Cargo metadata bake.release.tag-prefix must be a string"))?,
        None => "v",
    };
    if prefix.starts_with('-')
        || !prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(Error::new(
            "release tag prefix must contain only ASCII letters, numbers, hyphens, or underscores, and must not start with a hyphen",
        ));
    }
    Ok(prefix.to_owned())
}

#[cfg(test)]
mod tests {
    use super::release_tag;
    use crate::test_support::Project;

    #[test]
    fn defaults_to_standard_version_tags() {
        let project = Project::new();
        project.single_package("fixture", "0.1.0");
        assert_eq!(release_tag(&project.context(), "0.1.0").unwrap(), "v0.1.0");
    }

    #[test]
    fn workspace_configuration_takes_precedence_over_package_configuration() {
        let project = Project::new();
        project.write(
            "Cargo.toml",
            "[workspace.metadata.bake.release]\ntag-prefix = 'base-v'\n[package.metadata.bake.release]\ntag-prefix = 'package-v'\n",
        );
        assert_eq!(
            release_tag(&project.context(), "0.1.0").unwrap(),
            "base-v0.1.0"
        );
    }

    #[test]
    fn accepts_package_metadata_and_an_empty_prefix() {
        let project = Project::new();
        project.write(
            "Cargo.toml",
            "[package.metadata.bake.release]\ntag-prefix = 'base-v'\n",
        );
        assert_eq!(
            release_tag(&project.context(), "0.1.0").unwrap(),
            "base-v0.1.0"
        );
        project.write(
            "Cargo.toml",
            "[package.metadata.bake.release]\ntag-prefix = ''\n",
        );
        assert_eq!(release_tag(&project.context(), "0.1.0").unwrap(), "0.1.0");
    }

    #[test]
    fn absent_optional_metadata_uses_the_default_prefix() {
        let project = Project::new();
        for manifest in [
            "[package.metadata]",
            "[package.metadata.bake]",
            "[package.metadata.bake.release]",
        ] {
            project.write("Cargo.toml", manifest);
            assert_eq!(release_tag(&project.context(), "0.1.0").unwrap(), "v0.1.0");
        }
    }

    #[test]
    fn rejects_invalid_configuration_before_publication() {
        let project = Project::new();
        assert!(release_tag(&project.context(), "0.1.0").is_err());
        project.write("Cargo.toml", "[invalid");
        assert!(release_tag(&project.context(), "0.1.0").is_err());
        for value in ["42", "'-v'", "'../v'", "'release v'", "'v\u{0101}'"] {
            project.write(
                "Cargo.toml",
                &format!("[workspace.metadata.bake.release]\ntag-prefix = {value}\n"),
            );
            assert!(release_tag(&project.context(), "0.1.0").is_err(), "{value}");
        }
        project.single_package("fixture", "0.1.0");
        assert!(release_tag(&project.context(), "invalid").is_err());
    }
}
