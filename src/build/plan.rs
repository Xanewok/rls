use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use cargo::util::{process, ProcessBuilder};
use serde_derive::Deserialize;

use super::cargo_plan::WorkStatus;

trait BuildKey {
    type Key: Eq + Hash;
    fn key(&self) -> Self::Key;
}

trait BuildGraph {
    type Unit: BuildKey;

    fn units(&self) -> Vec<&Self::Unit>;
    fn get(&self, key: <Self::Unit as BuildKey>::Key) -> Option<&Self::Unit>;
    fn get_mut(&mut self, key: <Self::Unit as BuildKey>::Key) -> Option<&mut Self::Unit>;

    fn deps(&self, key: <Self::Unit as BuildKey>::Key) -> Vec<&Self::Unit>;
    // TODO: fn emplace_dep

    fn dirties<T: AsRef<Path>>(&self, files: &[T]) -> Vec<&Self::Unit>;
}

#[derive(Debug, Deserialize)]
struct RawPlan {
    invocations: Vec<RawInvocation>,
}

#[derive(Debug, Deserialize)]
struct RawInvocation {
    deps: Vec<usize>,
    outputs: Vec<PathBuf>,
    #[serde(default)]
    links: BTreeMap<PathBuf, PathBuf>,
    program: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    #[serde(default)]
    cwd: Option<PathBuf>,
}

crate struct DummyPlan {
    invocations: Vec<Invocation>,
}

#[derive(Debug)]
crate struct Invocation {
    deps: Vec<usize>, // FIXME: Use arena and store refs instead for ergonomics
    outputs: Vec<PathBuf>,
    links: BTreeMap<PathBuf, PathBuf>,
    command: ProcessBuilder,
    // Parsed data
    src_path: Option<PathBuf>,
}

/// Safe build plan type, invocation dependencies are guaranteed to be inside
/// the plan.
#[derive(Debug)]
struct BuildPlan {
    units: HashMap<u64, Invocation>,
    deps: HashMap<u64, HashSet<u64>>,
    rev_deps: HashMap<u64, HashSet<u64>>,
}

impl BuildKey for Invocation {
    type Key = u64;

    // Invocation key is the hash of the program, its arguments and environment.
    fn key(&self) -> u64 {
        let mut hash = DefaultHasher::new();

        self.command.get_program().hash(&mut hash);
        let /*mut*/ args = self.command.get_args().to_owned();
        // args.sort(); // TODO: Parse 2-part args (e.g. ["--extern", "a=b"])
        args.hash(&mut hash);
        let mut envs: Vec<_> = self.command.get_envs().iter().collect();
        envs.sort();
        envs.hash(&mut hash);

        hash.finish()
    }
}

impl Invocation {
    fn deps<'a>(&'a self, plan: &'a DummyPlan) -> impl Iterator<Item = &'a Invocation> {
        self.deps.iter().map(move |d| &plan.invocations[*d])
    }
}

impl From<RawInvocation> for Invocation {
    fn from(raw: RawInvocation) -> Invocation {
        let mut command = process(&raw.program);
        command.args(&raw.args);
        for (k, v) in &raw.env {
            command.env(&k, v);
        }
        if let Some(cwd) = &raw.cwd {
            command.cwd(cwd);
        }

        Invocation {
            deps: raw.deps.to_owned(),
            outputs: raw.outputs.to_owned(),
            links: raw.links.to_owned(),
            src_path: guess_rustc_src_path(&command),
            command,
        }
    }
}

impl DummyPlan {
    fn try_from_raw(raw: RawPlan) -> Result<DummyPlan, ()> {
        // Sanity check, each dependency (index) has to be inside the build plan
        if raw
            .invocations
            .iter()
            .flat_map(|inv| &inv.deps)
            .any(|idx| raw.invocations.get(*idx).is_none())
        {
            return Err(());
        }

        Ok(DummyPlan {
            invocations: raw.invocations.into_iter().map(|x| x.into()).collect(),
        })
    }
}

impl BuildPlan {
    fn try_from_raw(raw: RawPlan) -> Result<BuildPlan, ()> {
        // Sanity check, each dependency (index) has to be inside the build plan
        if raw
            .invocations
            .iter()
            .flat_map(|inv| &inv.deps)
            .any(|idx| raw.invocations.get(*idx).is_none())
        {
            return Err(());
        }

        let units: Vec<Invocation> = raw.invocations.into_iter().map(|x| x.into()).collect();

        let mut rev_deps = HashMap::new();
        let deps = units.iter().map(|unit| {
            let unit_deps = unit.deps.iter().map(|&idx| {
                let key = units[idx].key();
                rev_deps.entry(unit.key()).or_insert_with(HashSet::new).insert(key);
                key
            }).collect();
            (unit.key(), unit_deps)
        }).collect();
        let units = units.into_iter().map(|x| (x.key(), x)).collect();

        Ok(BuildPlan {
            units,
            deps,
            rev_deps,
        })
    }
}

impl BuildGraph for BuildPlan {
    type Unit = Invocation;

    fn units(&self) -> Vec<&Self::Unit> {
        self.units.values().collect()
    }

    fn get(&self, key: u64) -> Option<&Self::Unit> {
        self.units.get(&key)
    }

    fn get_mut(&mut self, key: u64) -> Option<&mut Self::Unit> {
        self.units.get_mut(&key)
    }

    fn deps(&self, key: u64) -> Vec<&Self::Unit> {
        self.deps
            .get(&key)
            .map(|d| d.iter().map(|d| &self.units[d]).collect())
            .unwrap_or_default()
    }

    // FIXME: Change associating files with units by their path but rather
    // include file inputs in the build plan or call rustc with --emit=dep-info
    fn dirties<T: AsRef<Path>>(&self, modified: &[T]) -> Vec<&Self::Unit> {
        let mut results = HashSet::<u64>::new();

        for modified in modified.iter().map(|x| x.as_ref()) {
            // We associate a dirty file with a
            // package by finding longest (most specified) path prefix.
            let matching_prefix_components = |a: &Path, b: &Path| -> usize {
                assert!(a.is_absolute() && b.is_absolute());
                a.components().zip(b.components())
                    .take_while(|&(x, y)| x == y)
                    .count()
            };
            // Since a package can correspond to many units (e.g. compiled
            // as a regular binary or a test harness for unit tests), we
            // collect every unit having the longest path prefix.
            let matching_units: Vec<(&_, usize)> = self.units.values()
                // For `rustc dir/some.rs` we'll consider every changed files
                // under dir/ as relevant
                .map(|unit| (unit, unit.src_path.as_ref().and_then(|src| src.parent())))
                .filter_map(|(unit, src)| src.map(|src| (unit, src)))
                // Discard units that are in a different directory subtree
                .filter_map(|(unit, src)| {
                    let matching = matching_prefix_components(modified, &src);
                    if matching >= src.components().count() {
                        Some((unit, matching))
                    } else {
                        None
                    }
                })
                .collect();

            // Changing files in the same directory might affect multiple units
            // (e.g. multiple crate binaries, their unit test harness), so
            // treat all of them as dirty.
            if let Some(max_prefix) = matching_units.iter().map(|(_, p)| p).max() {
                let dirty_keys = matching_units.iter()
                    .filter(|(_, prefix)| prefix == max_prefix)
                    .map(|(unit, _)| unit.key());

                results.extend(dirty_keys);
            }
        }

        results.iter().map(|key| &self.units[key]).collect()
    }
}

fn guess_rustc_src_path(cmd: &ProcessBuilder) -> Option<PathBuf> {
    // FIXME: Needle API for OsString can't come soon enough - this function
    // will be called often so it'd be great to avoid the transcoding overhead
    if !cmd.get_program().to_string_lossy().ends_with("rustc") {
        return None;
    }

    let file = cmd
        .get_args()
        .iter()
        .find(|&a| a.to_string_lossy().ends_with(".rs"))?;
    let file_path = PathBuf::from(file);

    Some(match (cmd.get_cwd(), file_path.is_absolute()) {
        (_, true) => file_path,
        (Some(cwd), _) => cwd.join(file_path),
        // TODO: is cwd correct here?
        (None, _) => std::env::current_dir().ok()?.join(file_path),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    /// Helper struct that prints sorted unit source directories in a given plan.
    struct SrcPaths<'a>(Vec<&'a PathBuf>);
    impl<'a> SrcPaths<'a> {
        fn from(plan: &BuildPlan) -> SrcPaths<'_> {
            SrcPaths(plan.units().iter().filter_map(|u| u.src_path.as_ref()).collect())
        }
    }

    impl<'a> fmt::Display for SrcPaths<'a> {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            let mut sorted = self.0.clone();
            sorted.sort();
            writeln!(f, "[")?;
            for src_path in sorted {
                write!(f, "  {}, ", src_path.display())?;
            }
            writeln!(f, "]")?;
            Ok(())
        }
    }

    #[test]
    fn dirty_units_path_heuristics() {
        let plan = r#"{"invocations": [
            { "deps": [],  "program": "rustc", "args": ["--crate-name", "build_script_build", "/my/repo/build.rs"], "env": {}, "outputs": [] },
            { "deps": [0], "program": "rustc", "args": ["--crate-name", "repo", "/my/repo/src/lib.rs"], "env": {}, "outputs": [] }
        ]}"#;
        let plan = serde_json::from_str::<RawPlan>(&plan).unwrap();
        let plan = BuildPlan::try_from_raw(plan).unwrap();

        eprintln!("src_paths: {}", &SrcPaths::from(&plan));

        let dirties = |file: &str| -> Vec<&str> {
            plan.dirties(&[file])
                .iter()
                .filter_map(|d| d.src_path.as_ref())
                .map(|p| p.to_str().unwrap())
                .collect()
        };

        assert_eq!(dirties("/my/dummy.rs"), Vec::<&str>::new());
        assert_eq!(dirties("/my/repo/dummy.rs"), vec!["/my/repo/build.rs"]);
        assert_eq!(dirties("/my/repo/src/dummy.rs"), vec!["/my/repo/src/lib.rs"]);
    }
}
