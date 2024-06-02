use std::collections::{BTreeMap, HashSet};

use cargo_toml::{Dependency, DependencyDetail};
use semver::VersionReq;

pub(crate) trait DependencyExt {
    fn simplify(&mut self);
    /// Merge a simple dependency into the current dependency.
    fn merge_simple(&mut self, version: &str);

    /// Merge a detailed dependency into the current dependency.
    fn merge_detailed(&mut self, details: Box<DependencyDetail>);
}

impl DependencyExt for Dependency {
    // VersionReq intersection is not a simple problem,
    // so we solve it the lazy way — adding everything together

    // TODO:
    // * handle git and similar versions

    fn merge_simple(&mut self, version: &str) {
        match self {
            Dependency::Simple(v) => {
                v.push_str(", ");
                v.push_str(&version);
            }
            Dependency::Detailed(ref mut detailed) => {
                if let Some(v) = &mut detailed.version {
                    v.push_str(", ");
                    v.push_str(&version);
                }
            }
            Dependency::Inherited(_) => {
                unreachable!("inherited dependencies are not supported")
            }
        }
    }

    fn merge_detailed(&mut self, mut details: Box<DependencyDetail>) {
        match self {
            Dependency::Simple(version) => {
                if let Some(detail_version) = &mut details.version {
                    // We push to original version to keep the constraint ordering,
                    // for the user, it makes more sense to append to the existing version
                    // rather than to the detailed version
                    version.push_str(", ");
                    version.push_str(&detail_version);
                    std::mem::swap(version, detail_version);
                }
                *self = Dependency::Detailed(details);
            }
            Dependency::Detailed(d) => match (&mut d.version, details.version) {
                (None, version @ Some(_)) => {
                    d.version = version;
                }
                (Some(l), Some(r)) => {
                    l.push_str(", ");
                    l.push_str(&r);
                }
                _ => { /* no-op */ }
            },
            Dependency::Inherited(_) => {
                unreachable!("inherited dependencies are not supported")
            }
        }
    }

    fn simplify(&mut self) {
        match self {
            Dependency::Simple(version) => {
                let mut version_req =
                    VersionReq::parse(version).expect("version requirement should be valid");
                version_req.simplify_version_req();
                *version = version_req.to_string();
            }
            Dependency::Detailed(details) => {
                if let Some(version) = &mut details.version {
                    let mut version_req =
                        VersionReq::parse(version).expect("version requirement should be valid");
                    version_req.simplify_version_req();
                    *version = version_req.to_string();
                }
            }
            Dependency::Inherited(_) => unreachable!("inherited dependencies are not supported"),
        }
    }
}

/// Unify multiple dependency versions into a single one.
/// Mostly done by joining constraints.
pub(crate) fn unify_dependencies(
    tree: BTreeMap<String, Vec<Dependency>>,
) -> BTreeMap<String, Dependency> {
    let mut unified_new_dependencies = BTreeMap::new();
    for (name, mut dependencies) in tree {
        let mut acc = dependencies
            .pop()
            .expect("dependency vector should not be empty");

        for dependency in dependencies {
            match (&mut acc, dependency) {
                (Dependency::Simple(_), Dependency::Simple(version))
                | (Dependency::Detailed(_), Dependency::Simple(version)) => {
                    acc.merge_simple(&version);
                }
                (Dependency::Simple(_), Dependency::Detailed(details))
                | (Dependency::Detailed(_), Dependency::Detailed(details)) => {
                    acc.merge_detailed(details)
                }
                (_, Dependency::Inherited(_)) | (Dependency::Inherited(_), _) => {
                    // If we reach here, it means that a dependency has `workspace = true`
                    // but at the same time, it isn't present in the main workspace file.
                    // Meaning the build is broken and the project shouldn't even compile.
                    // While this is fixable, for now, I'm not expecting this to be a problem.
                    // The easiest fix is to compile the whole workspace beforehand eheheh
                    unreachable!()
                }
            }
        }
        unified_new_dependencies.insert(name, acc);
    }
    unified_new_dependencies
}

pub(crate) trait VersionReqExt {
    /// Simplify a [`VersionReq`].
    fn simplify_version_req(&mut self);
}

impl VersionReqExt for VersionReq {
    fn simplify_version_req(&mut self) {
        self.comparators = std::mem::take(&mut self.comparators)
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<_>();

        // TODO: simplify further
    }
}

#[cfg(test)]
mod tests {
    use cargo_toml::{Dependency, DependencyDetail};

    use crate::dependencies::DependencyExt;

    #[test]
    fn simple_simple() {
        let mut original = Dependency::Simple("1.0.0".to_string());
        original.merge_simple("1.9.0");
        assert!(matches!(original, Dependency::Simple(_)));
        if let Dependency::Simple(version) = original {
            assert_eq!(version, "1.0.0, 1.9.0");
        }
    }

    #[test]
    fn detailed_simple() {
        let mut original = Dependency::Detailed(Box::new(DependencyDetail {
            version: Some("1.0.0".to_string()),
            ..Default::default()
        }));
        original.merge_simple("1.9.0");
        assert!(matches!(original, Dependency::Detailed(_)));
        if let Dependency::Detailed(details) = original {
            assert_eq!(details.version, Some("1.0.0, 1.9.0".to_string()));
        }
    }

    #[test]
    fn simple_detailed() {
        let mut original = Dependency::Simple("1.0.0".to_string());
        original.merge_detailed(Box::new(DependencyDetail {
            version: Some("1.9.0".to_string()),
            ..Default::default()
        }));
        assert!(matches!(original, Dependency::Detailed(_)));
        if let Dependency::Detailed(details) = original {
            assert_eq!(details.version, Some("1.0.0, 1.9.0".to_string()));
        }
    }
}
