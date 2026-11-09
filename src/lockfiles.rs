// Released under the MIT License.
// Copyright, 2026, by Samuel Williams.

//! Version refreshes for the lockfiles of nested workspaces.
//!
//! A repository can hold Cargo workspaces besides the released one, such as benchmarks or fuzz targets excluded from it, each with its own `Cargo.lock`. When those depend on released packages by path, their lockfiles record the packages' versions, so a version bump leaves them stale and `--locked` builds fail until they are refreshed.

use bake::{Context, Error, Result};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Value as TomlValue, value};

/// A package from `cargo metadata --no-deps`: its name, manifest, and the directories of its path dependencies.
pub(crate) struct ManifestPackage {
    pub name: String,
    pub manifest_path: PathBuf,
    pub path_dependencies: Vec<PathBuf>,
}

pub(crate) struct Metadata {
    pub workspace_root: PathBuf,
    pub packages: Vec<ManifestPackage>,
}

/// Read the workspace's packages without resolving dependencies, which leaves every lockfile untouched. Without a manifest path, Cargo uses the workspace containing the context root.
pub(crate) fn metadata(context: &Context, manifest_path: Option<&Path>) -> Result<Metadata> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = context.command(cargo);
    command.args(["metadata", "--format-version", "1", "--no-deps"]);
    if let Some(manifest_path) = manifest_path {
        command.arg("--manifest-path").arg(manifest_path);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(Error::new(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| Error::new(format!("could not parse Cargo metadata: {error}")))?;
    let workspace_root = metadata
        .get("workspace_root")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Error::new("Cargo metadata did not contain a workspace root"))?;
    let packages = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| Error::new("Cargo metadata did not contain a packages array"))?;

    let mut result = Vec::new();
    for package in packages {
        let manifest_path = package
            .get("manifest_path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::new("Cargo metadata package has no manifest path"))?;
        let name = package
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::new("Cargo metadata package has no name"))?;
        let path_dependencies = package
            .get("dependencies")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|dependency| dependency.get("path").and_then(serde_json::Value::as_str))
            .map(PathBuf::from)
            .collect();
        result.push(ManifestPackage {
            name: name.to_owned(),
            manifest_path: PathBuf::from(manifest_path),
            path_dependencies,
        });
    }

    Ok(Metadata {
        workspace_root: PathBuf::from(workspace_root),
        packages: result,
    })
}

/// Plan the new contents of every other tracked `Cargo.lock` in the repository that records a bumped package of `workspace` by path at `current`.
///
/// `bumped` holds the manifests of the workspace packages whose version changes from `current` to `target`. A lockfile is refreshed only when the workspace that uses it reaches one of those manifests through path dependencies, followed through that workspace's members and the released workspace's members, so a copy of a package kept elsewhere in the repository under the same name and version is left alone. Only the matching packages' `version` fields and the dependency references that name those versions change; every other entry, including registry versions and checksums, is kept byte for byte. Lockfiles whose workspace Cargo cannot read, and untracked lockfiles, are not changed.
pub(crate) fn plan(
    context: &Context,
    workspace: &Metadata,
    bumped: &HashSet<PathBuf>,
    current: &str,
    target: &str,
) -> Result<Vec<(PathBuf, String)>> {
    let mut bumped_directories = HashMap::new();
    let mut dependencies = HashMap::new();
    for package in &workspace.packages {
        let directory = manifest_directory(&package.manifest_path);
        if bumped.contains(&package.manifest_path) {
            bumped_directories.insert(directory.clone(), package.name.clone());
        }
        dependencies.insert(directory, canonical_paths(&package.path_dependencies));
    }
    let bumped_names: HashSet<_> = bumped_directories.values().cloned().collect();
    let root_lockfile = canonical(&workspace.workspace_root.join("Cargo.lock"));

    let mut planned = Vec::new();
    for directory in lockfile_directories(context)? {
        let lockfile = directory.join("Cargo.lock");
        if canonical(&lockfile) == root_lockfile {
            continue;
        }
        if let Some(contents) = plan_lockfile(
            context,
            &directory,
            &bumped_directories,
            &bumped_names,
            &dependencies,
            current,
            target,
        ) {
            planned.push((lockfile, contents));
        }
    }
    Ok(planned)
}

fn plan_lockfile(
    context: &Context,
    directory: &Path,
    bumped_directories: &HashMap<PathBuf, String>,
    bumped_names: &HashSet<String>,
    workspace_dependencies: &HashMap<PathBuf, Vec<PathBuf>>,
    current: &str,
    target: &str,
) -> Option<String> {
    let mut document: DocumentMut = fs::read_to_string(directory.join("Cargo.lock"))
        .ok()?
        .parse()
        .ok()?;
    let candidates: HashSet<String> = path_packages(&document)
        .filter(|(name, version)| version == current && bumped_names.contains(name))
        .map(|(name, _)| name)
        .collect();
    if candidates.is_empty() {
        return None;
    }

    // Cargo uses a lockfile only beside the root manifest of a workspace.
    let nested = metadata(context, Some(&directory.join("Cargo.toml"))).ok()?;
    if canonical(&nested.workspace_root) != canonical(directory) {
        return None;
    }

    let mut dependencies = workspace_dependencies.clone();
    let mut pending = Vec::new();
    for package in &nested.packages {
        let directory = manifest_directory(&package.manifest_path);
        pending.push(directory.clone());
        dependencies.insert(directory, canonical_paths(&package.path_dependencies));
    }
    let mut visited = BTreeSet::new();
    let mut referenced = HashSet::new();
    while let Some(directory) = pending.pop() {
        if !visited.insert(directory.clone()) {
            continue;
        }
        if let Some(name) = bumped_directories.get(&directory)
            && candidates.contains(name)
        {
            referenced.insert(name.clone());
        }
        if let Some(next) = dependencies.get(&directory) {
            pending.extend(next.iter().cloned());
        }
    }
    if referenced.is_empty() {
        return None;
    }

    refresh_versions(&mut document, &referenced, current, target);
    Some(document.to_string())
}

/// The name and version of each lockfile entry without a `source`, which Cargo writes for workspace members and path dependencies.
fn path_packages(document: &DocumentMut) -> impl Iterator<Item = (String, String)> + '_ {
    document
        .get("package")
        .and_then(Item::as_array_of_tables)
        .into_iter()
        .flatten()
        .filter(|package| !package.contains_key("source"))
        .filter_map(|package| {
            match (
                package.get("name").and_then(Item::as_str),
                package.get("version").and_then(Item::as_str),
            ) {
                (Some(name), Some(version)) => Some((name.to_owned(), version.to_owned())),
                _ => None,
            }
        })
}

fn refresh_versions(
    document: &mut DocumentMut,
    names: &HashSet<String>,
    current: &str,
    target: &str,
) {
    let references: HashMap<String, String> = names
        .iter()
        .map(|name| (format!("{name} {current}"), format!("{name} {target}")))
        .collect();
    let packages = document
        .get_mut("package")
        .and_then(Item::as_array_of_tables_mut)
        .into_iter()
        .flat_map(|packages| packages.iter_mut());
    for package in packages {
        let matches = !package.contains_key("source")
            && package.get("version").and_then(Item::as_str) == Some(current)
            && package
                .get("name")
                .and_then(Item::as_str)
                .is_some_and(|name| names.contains(name));
        if matches {
            package["version"] = value(target);
        }

        // Cargo writes `name version` only when several packages share a name; a reference to a path package never carries a source.
        let dependencies = package
            .get_mut("dependencies")
            .and_then(Item::as_array_mut)
            .into_iter()
            .flat_map(|dependencies| dependencies.iter_mut());
        for dependency in dependencies {
            if let Some(replacement) = dependency.as_str().and_then(|text| references.get(text)) {
                let decor = dependency.decor().clone();
                *dependency = TomlValue::from(replacement.as_str());
                *dependency.decor_mut() = decor;
            }
        }
    }
}

/// The directories of every `Cargo.lock` tracked in the Git repository containing the context root, or none outside a repository.
fn lockfile_directories(context: &Context) -> Result<Vec<PathBuf>> {
    let top = match context
        .command("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
    {
        Ok(output) if output.status.success() => {
            PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
        }
        _ => return Ok(Vec::new()),
    };
    let listed = match context
        .command("git")
        .current_dir(&top)
        .args(["ls-files", "-z", "--", ":(glob)**/Cargo.lock"])
        .output()
    {
        Ok(output) if output.status.success() => output.stdout,
        _ => {
            return Err(Error::new(
                "could not list the repository's tracked Cargo.lock files with git ls-files",
            ));
        }
    };
    Ok(listed
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            let mut directory = top.join(String::from_utf8_lossy(path).as_ref());
            directory.pop();
            directory
        })
        .collect())
}

fn manifest_directory(manifest_path: &Path) -> PathBuf {
    canonical(manifest_path.parent().unwrap_or(manifest_path))
}

fn canonical_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths.iter().map(|path| canonical(path)).collect()
}

/// Compare paths as the filesystem resolves them, since Cargo and Git may report the same directory through different symbolic links.
fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Environment, Project};
    use std::process::Command;

    fn git(root: &Path, arguments: &[&str]) {
        let status = Command::new("git")
            .current_dir(root)
            .args(arguments)
            .status()
            .unwrap();
        assert!(status.success(), "git {arguments:?} failed");
    }

    /// Run the real Cargo that runs these tests, which sets `CARGO`, keeping its output out of the test report.
    /// Run the real Cargo that runs these tests, which sets `CARGO`, keeping its output out of the test report.
    fn cargo(environment: &Environment, arguments: &[&str]) -> bool {
        Command::new(environment.original("CARGO").unwrap())
            .args(arguments)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    fn generate_lockfile(environment: &Environment, manifest: &Path) {
        assert!(cargo(
            environment,
            &[
                "generate-lockfile",
                "--offline",
                "--manifest-path",
                manifest.to_str().unwrap(),
            ],
        ));
    }

    fn nested(project: &Project, directory: &str, name: &str, dependencies: &str) -> PathBuf {
        project.write(format!("{directory}/src/lib.rs"), "// fixture\n");
        project.write(
            format!("{directory}/Cargo.toml"),
            &format!(
                "[package]\nname = {name:?}\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[workspace]\n\n[dependencies]\n{dependencies}"
            ),
        )
    }

    const STALE: &str = "version = 4\n\n[[package]]\nname = \"alpha\"\nversion = \"1.2.3\"\n";

    /// A released workspace whose repository also holds nested workspaces, copies, and stray or broken lockfiles.
    fn repository(environment: &mut Environment) -> Project {
        for name in ["GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"] {
            environment.set(name, "Fixture");
        }
        for name in ["GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL"] {
            environment.set(name, "fixture@example.com");
        }
        environment.set("GIT_CONFIG_GLOBAL", "/dev/null");
        environment.set("GIT_CONFIG_NOSYSTEM", "1");

        let project = Project::new();
        project.write(
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/alpha\", \"crates/beta\", \"crates/support\", \"bake\"]\nresolver = \"3\"\n\n[workspace.package]\nversion = \"1.2.3\"\nedition = \"2024\"\n\n[workspace.dependencies]\nbeta = { path = \"crates/beta\", version = \"1.2.3\" }\n",
        );
        project.write(
            "crates/alpha/Cargo.toml",
            "[package]\nname = \"alpha\"\nversion.workspace = true\nedition.workspace = true\n\n[dependencies]\nbeta.workspace = true\n",
        );
        project.write(
            "crates/beta/Cargo.toml",
            "[package]\nname = \"beta\"\nversion.workspace = true\nedition.workspace = true\n",
        );
        // An unpublished member that inherits the workspace version changes with it.
        project.write(
            "crates/support/Cargo.toml",
            "[package]\nname = \"support\"\nversion.workspace = true\nedition.workspace = true\npublish = false\n\n[dependencies]\nalpha = { path = \"../alpha\" }\n",
        );
        project.write(
            "bake/Cargo.toml",
            "[package]\nname = \"build-tools\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n[dependencies]\nalpha = { path = \"../crates/alpha\", version = \"1.2.3\" }\n",
        );
        for directory in ["crates/alpha", "crates/beta", "crates/support", "bake"] {
            project.write(format!("{directory}/src/lib.rs"), "// fixture\n");
        }
        generate_lockfile(environment, &project.root().join("Cargo.toml"));

        // A standalone path dependency outside every workspace, whose own dependencies are not followed.
        project.write(
            "vendor/helper/Cargo.toml",
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
        );
        project.write("vendor/helper/src/lib.rs", "// fixture\n");
        // Nested workspaces reaching released packages directly, through an unpublished member that inherits the version, and through one with its own version.
        for (directory, name, dependencies) in [
            (
                "benchmarks",
                "benchmarks",
                "alpha = { path = \"../crates/alpha\" }\nbeta = { path = \"../crates/beta\" }\n",
            ),
            (
                "fuzz",
                "fuzz",
                "support = { path = \"../crates/support\" }\n",
            ),
            (
                "tools",
                "tools",
                "build-tools = { path = \"../bake\" }\nhelper = { path = \"../vendor/helper\" }\n",
            ),
            (
                "untracked",
                "untracked",
                "alpha = { path = \"../crates/alpha\" }\n",
            ),
        ] {
            let manifest = nested(&project, directory, name, dependencies);
            generate_lockfile(environment, &manifest);
        }
        // A copy of a package with a released name and version, kept as evidence.
        let copy = project.write(
            "results/copy/Cargo.toml",
            "[package]\nname = \"alpha\"\nversion = \"1.2.3\"\nedition = \"2024\"\n\n[workspace]\n",
        );
        project.write("results/copy/src/lib.rs", "// fixture\n");
        generate_lockfile(environment, &copy);
        // Lockfiles naming a released package that Cargo would not use or cannot read.
        project.write("crates/alpha/Cargo.lock", STALE);
        project.write("results/broken/Cargo.toml", "not TOML [\n");
        project.write("results/broken/Cargo.lock", STALE);
        project.write("results/no-manifest/Cargo.lock", STALE);
        project.write("results/invalid/Cargo.lock", "not TOML [\n");
        project.write(
            "results/partial/Cargo.lock",
            "version = 4\n\n[[package]]\nname = \"alpha\"\n",
        );
        project.write("results/deleted/Cargo.lock", STALE);

        git(
            project.root(),
            &["init", "--quiet", "--initial-branch=main"],
        );
        git(project.root(), &["add", ".", ":!untracked"]);
        git(
            project.root(),
            &["commit", "--quiet", "--message", "Initial commit."],
        );
        fs::remove_file(project.root().join("results/deleted/Cargo.lock")).unwrap();
        project
    }

    fn read(project: &Project, path: &str) -> String {
        fs::read_to_string(project.root().join(path)).unwrap()
    }

    #[test]
    fn refreshes_released_versions_in_nested_workspace_lockfiles() {
        let mut environment = Environment::new();
        let project = repository(&mut environment);
        let before: Vec<_> = [
            "benchmarks",
            "fuzz",
            "tools",
            "untracked",
            "results/copy",
            "crates/alpha",
            "results/broken",
            "results/no-manifest",
            "results/invalid",
            "results/partial",
        ]
        .into_iter()
        .map(|directory| {
            (
                directory,
                read(&project, &format!("{directory}/Cargo.lock")),
            )
        })
        .collect();
        project.cargo_proxy(&mut environment, None);

        let result = crate::version_support::increment(
            &project.context(),
            crate::version_support::Component::Patch,
        )
        .unwrap();

        let root = fs::canonicalize(project.root()).unwrap();
        let mut updated: Vec<_> = result["lockfiles_updated"]
            .as_array()
            .unwrap()
            .iter()
            .map(|path| {
                Path::new(path.as_str().unwrap())
                    .strip_prefix(&root)
                    .unwrap()
                    .display()
                    .to_string()
            })
            .collect();
        updated.sort();
        assert_eq!(
            updated,
            [
                "benchmarks/Cargo.lock",
                "fuzz/Cargo.lock",
                "tools/Cargo.lock"
            ]
        );

        for (directory, previous) in before {
            let current = read(&project, &format!("{directory}/Cargo.lock"));
            let bumped = match directory {
                "benchmarks" | "tools" => &["alpha", "beta"][..],
                "fuzz" => &["alpha", "beta", "support"][..],
                _ => &[][..],
            };
            let mut expected = previous;
            for name in bumped {
                expected = expected.replace(
                    &format!("name = \"{name}\"\nversion = \"1.2.3\"\n"),
                    &format!("name = \"{name}\"\nversion = \"1.2.4\"\n"),
                );
            }
            assert_eq!(current, expected, "{directory}");
        }
        assert!(!read(&project, "tools/Cargo.lock").contains("version = \"0.1.1\""));

        // Each refreshed lockfile is what Cargo itself would now write.
        for directory in ["benchmarks", "fuzz", "tools"] {
            let manifest = project.root().join(directory).join("Cargo.toml");
            let arguments = [
                "metadata",
                "--format-version",
                "1",
                "--locked",
                "--offline",
                "--manifest-path",
                manifest.to_str().unwrap(),
            ];
            assert!(cargo(&environment, &arguments), "{directory}");
        }
    }

    #[test]
    fn reports_lockfile_listing_failures_before_changing_manifests() {
        let mut environment = Environment::new();
        let project = repository(&mut environment);
        let manifest = read(&project, "Cargo.toml");
        project.cargo_proxy(&mut environment, None);
        project.git_proxy(&mut environment, Some("ls-files"));

        let error = crate::version_support::increment(
            &project.context(),
            crate::version_support::Component::Patch,
        )
        .unwrap_err();

        assert!(error.to_string().contains("git ls-files"), "{error}");
        assert_eq!(read(&project, "Cargo.toml"), manifest);
    }

    #[test]
    fn refreshes_versioned_references_to_path_packages_only() {
        let source = "# This file is automatically @generated by Cargo.\nversion = 4\n\n[[package]]\nname = \"alpha\"\nversion = \"1.0.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"00\"\n\n[[package]]\nname = \"alpha\"\nversion = \"1.2.3\"\ndependencies = [\n \"beta\",\n]\n\n[[package]]\nname = \"beta\"\nversion = \"1.2.3\"\n\n[[package]]\nname = \"gamma\"\nversion = \"1.2.3\"\n\n[[package]]\nname = \"user\"\nversion = \"0.0.0\"\ndependencies = [\n \"alpha 1.0.0\",\n \"alpha 1.2.3\",\n \"gamma\",\n]\n";
        let mut document: DocumentMut = source.parse().unwrap();
        refresh_versions(
            &mut document,
            &HashSet::from(["alpha".to_owned(), "beta".to_owned()]),
            "1.2.3",
            "1.2.4",
        );
        let expected = source
            .replace(
                "name = \"alpha\"\nversion = \"1.2.3\"",
                "name = \"alpha\"\nversion = \"1.2.4\"",
            )
            .replace(
                "name = \"beta\"\nversion = \"1.2.3\"",
                "name = \"beta\"\nversion = \"1.2.4\"",
            )
            .replace(" \"alpha 1.2.3\",", " \"alpha 1.2.4\",");
        assert_eq!(document.to_string(), expected);

        let mut empty = DocumentMut::new();
        refresh_versions(&mut empty, &HashSet::new(), "1.2.3", "1.2.4");
        assert_eq!(empty.to_string(), "");
    }
}
