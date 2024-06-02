mod dependencies;

use std::{collections::BTreeMap, env, path::PathBuf};

use cargo_toml::{Dependency, Manifest};
use clap::Parser;
use dependencies::{unify_dependencies, DependencyExt};
use serde::Serialize;
use thiserror::Error;
use toml_edit::{DocumentMut, Formatted, Item, Value};

fn default_cargo_path() -> PathBuf {
    // NOTE: ngl if it fails here, I don't know what to do
    let mut current_dir = env::current_dir().unwrap();
    current_dir.push("Cargo.toml");
    current_dir
}

/// Consolidate multiple package dependencies into a single workspace.
#[derive(Parser)]
struct App {
    /// Target Cargo.toml workspace to consolidate
    #[arg(default_value = default_cargo_path().into_os_string())]
    target: PathBuf,

    /// Consolidate even if the working directory is dirty
    #[arg(long, default_value_t = false)]
    allow_dirty: bool,

    /// Consolidate even if the working directory has staged changes
    #[arg(long, default_value_t = false)]
    allow_staged: bool,
}

#[derive(Error, Debug)]
enum ConsolidateError {
    #[error("no workspace was found in Cargo.toml")]
    NoWorkspace,
    #[error(transparent)]
    ManifestError(#[from] cargo_toml::Error),
    #[error(transparent)]
    SemverError(#[from] semver::Error),
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error(transparent)]
    TomlError(#[from] toml_edit::TomlError),
}

impl<T> From<ConsolidateError> for Result<T, ConsolidateError> {
    fn from(value: ConsolidateError) -> Self {
        Self::Err(value)
    }
}

impl App {
    fn consolidate(self, manifest: Manifest) -> Result<(), ConsolidateError> {
        let Some(workspace) = manifest.workspace else {
            return Err(ConsolidateError::NoWorkspace);
        };

        // Collect all workspace members
        let members = self.read_members(workspace.members)?;

        // Collect all their dependencies that are not in the workspace already
        // we will make them `workspace = true` later
        let mut new_dependencies: BTreeMap<_, Vec<Dependency>> = BTreeMap::new();
        for (_, manifest) in &members {
            for (name, dependency) in &manifest.dependencies {
                if workspace.dependencies.contains_key(name) {
                    // TODO: check for default-features and friends
                    // maybe we can do that later on too
                    continue;
                }
                // Keep track of dependencies with the same name but different version/sources
                if let Some(dependencies) = new_dependencies.get_mut(name) {
                    dependencies.push(dependency.clone());
                    // TODO: replace dependency version with workspace = true
                    // do it by going back LATER, this will avoid sourcing conflicts
                    // because we can just warn the user about them and not do shit
                } else {
                    new_dependencies.insert(name.clone(), vec![dependency.clone()]);
                }
            }
        }
        let mut unified_new_dependencies = unify_dependencies(new_dependencies);
        unified_new_dependencies
            .iter_mut()
            .for_each(|(_, dependency)| dependency.simplify());

        let cargo_toml_contents = std::fs::read_to_string(self.target)?;
        let mut editable_cargo_toml = cargo_toml_contents.parse::<DocumentMut>()?;

        if let Some(dependencies) = editable_cargo_toml
            .get_mut("workspace")
            .and_then(|workspace| {
                workspace
                    .get_mut("dependencies")
                    .and_then(|dependencies| dependencies.as_table_mut())
            })
        {
            for (name, dependency) in &unified_new_dependencies {
                let value = dependency
                    .serialize(toml_edit::ser::ValueSerializer::default())
                    .unwrap();
                dependencies.insert(&name, toml_edit::Item::Value(value));
            }
        }

        for (member_path, _) in &members {
            let member_cargo_toml = std::fs::read_to_string(member_path)?;
            let mut member = member_cargo_toml.parse::<DocumentMut>()?;
            let dependencies = member
                .get_mut("dependencies")
                .expect("dependencies should exist");
            let dependencies = dependencies
                .as_table_mut()
                .expect("dependencies should be in the correct format");

            for (name, value) in dependencies.iter_mut() {
                if let Some(dep) = unified_new_dependencies.get(name.display_repr().as_ref()) {
                    if value.is_str() {
                        match dep {
                            Dependency::Simple(version) => {
                                *value =
                                    Item::Value(Value::String(Formatted::new(version.clone())));
                            }
                            Dependency::Detailed(details) => {
                                let v = (details.version.as_ref()).expect("version should exist");
                                *value = Item::Value(Value::String(Formatted::new(v.clone())));
                            }
                            Dependency::Inherited(_) => { /* no-op */ }
                        }
                    } else if value.is_table_like() {
                        if let Some(version_field) = value.get_mut("version") {
                            match dep {
                                Dependency::Simple(version) => {
                                    let value = Value::String(Formatted::new(version.clone()));
                                    *version_field = Item::Value(value)
                                }
                                Dependency::Detailed(details) => {
                                    let v =
                                        (details.version.as_ref()).expect("version should exist");
                                    let value = Value::String(Formatted::new(v.clone()));
                                    *version_field = Item::Value(value)
                                }
                                Dependency::Inherited(_) => { /* no-op */ }
                            }
                        }
                    } else {
                        unimplemented!("{:?}", value)
                    }
                }
            }

            std::fs::write(member_path, member.to_string())?;
        }

        Ok(std::fs::write("test", editable_cargo_toml.to_string())?)
    }

    fn read_members(
        &self,
        members: Vec<String>,
    ) -> Result<BTreeMap<PathBuf, Manifest>, ConsolidateError> {
        Ok(members
            .into_iter()
            .map(|member| {
                let mut member_manifest_path = self.target.clone();
                member_manifest_path.pop();
                member_manifest_path.push(&member);
                member_manifest_path.push("Cargo.toml");
                Manifest::from_path(&member_manifest_path)
                    .map(|manifest| (member_manifest_path, manifest))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?)
    }
}
// TODO: re-architect so that App is just Args
fn main() -> Result<(), anyhow::Error> {
    let mut app = App::parse();
    // TODO: check if target is a directory — if yes, push Cargo.toml to the path
    app.target = app.target.canonicalize()?;

    if app.target.is_dir() {
        app.target.push("Cargo.toml");
    }

    println!("{:?}", app.target);

    if !app.target.exists() {
        // NOTE(@jmg-duarte,02/06/2024): There has to be a better way to write this
        return Err(ConsolidateError::IoError(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file/directory not found",
        ))
        .into());
    }

    let cargo_contents = Manifest::from_path(&app.target)?;
    Ok(app.consolidate(cargo_contents)?)
}
