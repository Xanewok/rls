// Copyright 2017 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! This contains a build plan that is created during the Cargo build routine
//! and stored afterwards, which can be later queried, given a list of dirty
//! files, to retrieve a queue of compiler calls to be invoked (including
//! appropriate arguments and env variables).
//! The underlying structure is a dependency graph between simplified units
//! (package id and crate target kind), as opposed to Cargo units (package with
//! a target info, including crate target kind, profile and host/target kind).
//! This will be used for a quick check recompilation and does not aim to
//! reimplement all the intricacies of Cargo.
//! The unit dependency graph in Cargo also distinguishes between compiling the
//! build script and running it and collecting the build script output to modify
//! the subsequent compilations etc. Since build script executions (not building)
//! are not exposed via `Executor` trait in Cargo, we simply coalesce every unit
//! with a same package and crate target kind (e.g. both building and running
//! build scripts).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cargo::core::compiler::{CompileMode, Context, Kind, Unit};
use cargo::core::profiles::Profile;
use cargo::core::{PackageId, Target, TargetKind};
use cargo::util::ProcessBuilder;
use cargo_metadata;
use log::{error, trace};
use url::Url;

use crate::build::PackageArg;
use crate::build::plan::{BuildKey, BuildGraph, JobQueue, WorkStatus};
use crate::lsp_data::parse_file_path;

/// Main key type by which `Unit`s will be distinguished in the build plan.
/// In Target we're mostly interested in TargetKind (Lib, Bin, ...) and name
/// (e.g. we can have 2 binary targets with different names).
crate type UnitKey = (PackageId, Target, CompileMode);

/// Holds the information how exactly the build will be performed for a given
/// workspace with given, specified features.
#[derive(Debug, Default)]
crate struct CargoPlan {
    /// Stores a full Cargo `Unit` data for a first processed unit with a given key.
    crate units: HashMap<UnitKey, OwnedUnit>,
    /// Main dependency graph between the simplified units.
    crate dep_graph: HashMap<UnitKey, HashSet<UnitKey>>,
    /// Reverse dependency graph that's used to construct a dirty compiler call queue.
    crate rev_dep_graph: HashMap<UnitKey, HashSet<UnitKey>>,
    /// Cached compiler calls used when creating a compiler call queue.
    crate compiler_jobs: HashMap<UnitKey, ProcessBuilder>,
    // An object for finding the package which a file belongs to and this inferring
    // a package argument.
    package_map: Option<PackageMap>,
    /// Packages (names) for which this build plan was prepared.
    /// Used to detect if the plan can reused when building certain packages.
    built_packages: HashSet<String>,
}

impl CargoPlan {
    crate fn with_manifest(manifest_path: &Path) -> CargoPlan {
        CargoPlan {
            package_map: Some(PackageMap::new(manifest_path)),
            ..Default::default()
        }
    }

    crate fn with_packages(manifest_path: &Path, pkgs: HashSet<String>) -> CargoPlan {
        CargoPlan {
            built_packages: pkgs,
            ..Self::with_manifest(manifest_path)
        }
    }

    /// Returns whether a build plan has cached compiler invocations and dep
    /// graph so it's at all able to return a job queue via `prepare_work`.
    crate fn is_ready(&self) -> bool {
        !self.compiler_jobs.is_empty()
    }

    /// Cache a given compiler invocation in `ProcessBuilder` for a given
    /// `PackageId` and `TargetKind` in `Target`, to be used when processing
    /// cached build plan.
    crate fn cache_compiler_job(
        &mut self,
        id: &PackageId,
        target: &Target,
        mode: CompileMode,
        cmd: &ProcessBuilder,
    ) {
        let unit_key = (id.clone(), target.clone(), mode);
        self.compiler_jobs.insert(unit_key, cmd.clone());
    }

    /// Emplace a given `Unit`, along with its `Unit` dependencies (recursively)
    /// into the dependency graph as long as the passed `Unit` isn't filtered
    /// out by the `filter` closure.
    crate fn emplace_dep_with_filter<Filter>(
        &mut self,
        unit: &Unit<'_>,
        cx: &Context<'_, '_>,
        filter: &Filter,
    )
    where
        Filter: Fn(&Unit<'_>) -> bool,
    {
        if !filter(unit) {
            return;
        }

        let key = key_from_unit(unit);
        self.units.entry(key.clone()).or_insert_with(|| (*unit).into());
        // Process only those units, which are not yet in the dep graph.
        if self.dep_graph.get(&key).is_some() {
            return;
        }

        // Keep all the additional Unit information for a given unit (It's
        // worth remembering, that the units are only discriminated by a
        // pair of (PackageId, TargetKind), so only first occurrence will be saved.
        self.units.insert(key.clone(), (*unit).into());

        // Fetch and insert relevant unit dependencies to the forward dep graph.
        let units = cx.dep_targets(unit);
        let dep_keys: HashSet<UnitKey> = units.iter()
            // We might not want certain deps to be added transitively (e.g.
            // when creating only a sub-dep-graph, limiting the scope).
            .filter(|unit| filter(unit))
            .map(key_from_unit)
            // Units can depend on others with different Targets or Profiles
            // (e.g. different `run_custom_build`) despite having the same UnitKey.
            // We coalesce them here while creating the UnitKey dep graph.
            .filter(|dep| key != *dep)
            .collect();
        self.dep_graph.insert(key.clone(), dep_keys.clone());

        // We also need to track reverse dependencies here, as it's needed
        // to quickly construct a work sub-graph from a set of dirty units.
        self.rev_dep_graph
            .entry(key.clone())
            .or_insert_with(HashSet::new);
        for unit in dep_keys {
            let revs = self.rev_dep_graph.entry(unit).or_insert_with(HashSet::new);
            revs.insert(key.clone());
        }

        // Recursively process other remaining forward dependencies.
        for unit in units {
            self.emplace_dep_with_filter(&unit, cx, filter);
        }
    }

    /// TODO: Improve detecting dirty crate targets for a set of dirty file paths.
    /// This uses a lousy heuristic of checking path prefix for a given crate
    /// target to determine whether a given unit (crate target) is dirty. This
    /// can easily backfire, e.g. when build script is under src/. Any change
    /// to a file under src/ would imply the build script is always dirty, so we
    /// never do work and always offload to Cargo in such case.
    /// Because of that, build scripts are checked separately and only other
    /// crate targets are checked with path prefixes.
    fn fetch_dirty_units<T: AsRef<Path>>(&self, files: &[T]) -> HashSet<UnitKey> {
        let mut result = HashSet::new();

        let build_scripts: HashMap<&Path, UnitKey> = self
            .units
            .iter()
            .filter(|&(&(_, ref target, _), _)| {
                *target.kind() == TargetKind::CustomBuild && target.src_path().is_path()
            })
            .map(|(key, unit)| (unit.target.src_path().path(), key.clone()))
            .collect();
        let other_targets: HashMap<UnitKey, &Path> = self
            .units
            .iter()
            .filter(|&(&(_, ref target, _), _)| *target.kind() != TargetKind::CustomBuild)
            .map(|(key, unit)| {
                (
                    key.clone(),
                    unit.target
                        .src_path()
                        .path()
                        .parent()
                        .expect("no parent for src_path"),
                )
            }).collect();

        for modified in files.iter().map(|x| x.as_ref()) {
            if let Some(unit) = build_scripts.get(modified) {
                result.insert(unit.clone());
            } else {
                // Not a build script, so we associate a dirty file with a
                // package by finding longest (most specified) path prefix.
                let matching_prefix_components = |a: &Path, b: &Path| -> usize {
                    assert!(a.is_absolute() && b.is_absolute());
                    a.components().zip(b.components())
                        .skip(1) // Skip RootDir
                        .take_while(|&(x, y)| x == y)
                        .count()
                };
                // Since a package can correspond to many units (e.g. compiled
                // as a regular binary or a test harness for unit tests), we
                // collect every unit having the longest path prefix.
                let max_matching_prefix = other_targets
                    .values()
                    .map(|src_dir| matching_prefix_components(modified, src_dir))
                    .max();

                match max_matching_prefix {
                    Some(0) => error!(
                        "Modified file {} didn't correspond to any buildable unit!",
                        modified.display()
                    ),
                    Some(max) => {
                        let dirty_units = other_targets
                            .iter()
                            .filter(|(_, dir)| max == matching_prefix_components(modified, dir))
                            .map(|(unit, _)| unit);

                        result.extend(dirty_units.cloned());
                    }
                    None => {} // Possible that only build scripts were modified
                }
            }
        }
        result
    }

    /// For a given set of select dirty units, returns a set of all the
    /// dependencies that has to be rebuilt transitively.
    fn transitive_dirty_units(&self, dirties: &HashSet<UnitKey>) -> HashSet<UnitKey> {
        let mut transitive = dirties.clone();
        // Walk through a rev dep graph using a stack of nodes to collect
        // transitively every dirty node
        let mut to_process: Vec<_> = dirties.iter().cloned().collect();
        while let Some(top) = to_process.pop() {
            if transitive.get(&top).is_some() {
                continue;
            }
            transitive.insert(top.clone());

            // Process every dirty rev dep of the processed node
            let dirty_rev_deps = self
                .rev_dep_graph
                .get(&top)
                .expect("missing key in rev_dep_graph")
                .iter()
                .filter(|dep| dirties.contains(dep));
            for rev_dep in dirty_rev_deps {
                to_process.push(rev_dep.clone());
            }
        }
        transitive
    }

    /// Creates a dirty reverse dependency graph using a set of given dirty units.
    fn dirty_rev_dep_graph(
        &self,
        dirties: &HashSet<UnitKey>,
    ) -> HashMap<UnitKey, HashSet<UnitKey>> {
        let dirties = self.transitive_dirty_units(dirties);
        trace!("transitive_dirty_units: {:?}", dirties);

        self.rev_dep_graph.iter()
            // Remove nodes that are not dirty
            .filter(|&(unit, _)| dirties.contains(unit))
            // Retain only dirty dependencies of the ones that are dirty
            .map(|(k, deps)| (k.clone(), deps.iter().cloned().filter(|d| dirties.contains(d)).collect()))
            .collect()
    }

    /// Returns a topological ordering of a connected DAG of rev deps. The
    /// output is a stack of units that can be linearly rebuilt, starting from
    /// the last element.
    fn topological_sort(&self, dirties: &HashMap<UnitKey, HashSet<UnitKey>>) -> Vec<UnitKey> {
        let mut visited = HashSet::new();
        let mut output = vec![];

        for k in dirties.keys() {
            if !visited.contains(k) {
                dfs(k, &self.rev_dep_graph, &mut visited, &mut output);
            }
        }

        return output;

        // Process graph depth-first recursively. A node needs to be pushed
        // after processing every other before to ensure topological ordering.
        fn dfs(
            unit: &UnitKey,
            graph: &HashMap<UnitKey, HashSet<UnitKey>>,
            visited: &mut HashSet<UnitKey>,
            output: &mut Vec<UnitKey>,
        ) {
            if visited.contains(unit) {
                return;
            } else {
                visited.insert(unit.clone());
                for neighbour in graph.get(unit).into_iter().flat_map(|nodes| nodes) {
                    dfs(neighbour, graph, visited, output);
                }
                output.push(unit.clone());
            }
        }
    }

    crate fn prepare_work<T: AsRef<Path> + fmt::Debug>(
        &self,
        modified: &[T],
    ) -> WorkStatus {
        if !self.is_ready() || self.package_map.is_none() {
            return WorkStatus::NeedsCargo(PackageArg::Default);
        }

        let dirty_packages = self
            .package_map
            .as_ref()
            .unwrap()
            .compute_dirty_packages(modified);

        let needs_more_packages = dirty_packages
            .difference(&self.built_packages)
            .next()
            .is_some();

        let needed_packages = self
            .built_packages
            .union(&dirty_packages)
            .cloned()
            .collect();

        // We modified a file from a packages, that are not included in the
        // cached build plan - run Cargo to recreate the build plan including them
        if needs_more_packages {
            return WorkStatus::NeedsCargo(PackageArg::Packages(needed_packages));
        }

        let dirties = self.fetch_dirty_units(modified);
        trace!(
            "fetch_dirty_units: for files {:?}, these units are dirty: {:?}",
            modified,
            dirties,
        );

        if dirties
            .iter()
            .any(|&(_, ref target, _)| *target.kind() == TargetKind::CustomBuild)
        {
            WorkStatus::NeedsCargo(PackageArg::Packages(needed_packages))
        } else {
            let graph = self.dirty_rev_dep_graph(&dirties);
            trace!("Constructed dirty rev dep graph: {:?}", graph);

            if graph.is_empty() {
                return WorkStatus::NeedsCargo(PackageArg::Default);
            }

            let queue = self.topological_sort(&graph);
            trace!(
                "Topologically sorted dirty graph: {:?} {}",
                queue,
                self.is_ready()
            );
            let jobs: Option<Vec<_>> = queue
                .iter()
                .map(|x| self.compiler_jobs.get(x).cloned())
                .collect();

            // It is possible that we want a job which is not in our cache (compiler_jobs),
            // for example we might be building a workspace with an error in a crate and later
            // crates within the crate that depend on the error-ing one have never been built.
            // In that case we need to build from scratch so that everything is in our cache, or
            // we cope with the error. In the error case, jobs will be None.
            match jobs {
                None => WorkStatus::NeedsCargo(PackageArg::Default),
                Some(jobs) => {
                    assert!(!jobs.is_empty());
                    WorkStatus::Execute(JobQueue::with_commands(jobs))
                }
            }
        }
    }
}

/// Maps paths to packages.
///
/// The point of the PackageMap is detect if additional packages need to be
/// included in the cached build plan. The cache can represent only a subset of
/// the entire workspace, hence why we need to detect if a package was modified
/// that's outside the cached build plan - if so, we need to recreate it,
/// including the new package.
#[derive(Debug)]
struct PackageMap {
    // A map from a manifest directory to the package name.
    package_paths: HashMap<PathBuf, String>,
    // A map from a file's path, to the package it belongs to.
    map_cache: Mutex<HashMap<PathBuf, String>>,
}

impl PackageMap {
    fn new(manifest_path: &Path) -> PackageMap {
        PackageMap {
            package_paths: Self::discover_package_paths(manifest_path),
            map_cache: Mutex::new(HashMap::new()),
        }
    }

    // Find each package in the workspace and record the root directory and package name.
    fn discover_package_paths(manifest_path: &Path) -> HashMap<PathBuf, String> {
        trace!("read metadata {:?}", manifest_path);
        let metadata = match cargo_metadata::metadata(Some(manifest_path)) {
            Ok(metadata) => metadata,
            Err(_) => return HashMap::new(),
        };
        metadata
            .workspace_members
            .into_iter()
            .map(|wm| {
                assert!(wm.url().starts_with("path+"));
                let url = Url::parse(&wm.url()[5..]).expect("Bad URL");
                let path = parse_file_path(&url).expect("URL not a path");
                (path, wm.name().into())
            }).collect()
    }

    /// Given modified set of files, returns a set of corresponding dirty packages.
    fn compute_dirty_packages<T: AsRef<Path> + fmt::Debug>(
        &self,
        modified_files: &[T],
    ) -> HashSet<String> {
        modified_files
            .iter()
            .filter_map(|p| self.map(p.as_ref()))
            .collect()
    }

    // Map a file to the package which it belongs to.
    // We do this by walking up the directory tree from `path` until we get to
    // one of the recorded package root directories.
    fn map(&self, path: &Path) -> Option<String> {
        if self.package_paths.is_empty() {
            return None;
        }

        let mut map_cache = self.map_cache.lock().unwrap();
        if map_cache.contains_key(path) {
            return Some(map_cache[path].clone());
        }

        let result = Self::map_uncached(path, &self.package_paths)?;

        map_cache.insert(path.to_owned(), result.clone());
        Some(result)
    }

    fn map_uncached(path: &Path, package_paths: &HashMap<PathBuf, String>) -> Option<String> {
        if package_paths.is_empty() {
            return None;
        }

        match package_paths.get(path) {
            Some(package) => Some(package.clone()),
            None => Self::map_uncached(path.parent()?, package_paths),
        }
    }
}

fn key_from_unit(unit: &Unit<'_>) -> UnitKey {
    (
        unit.pkg.package_id().clone(),
        unit.target.clone(),
        unit.mode,
    )
}

#[derive(Hash, PartialEq, Eq, Debug, Clone)]
/// An owned version of `cargo::core::Unit`.
crate struct OwnedUnit {
    crate id: PackageId,
    crate target: Target,
    crate profile: Profile,
    crate kind: Kind,
    crate mode: CompileMode,
}

impl<'a> From<Unit<'a>> for OwnedUnit {
    fn from(unit: Unit<'a>) -> OwnedUnit {
        OwnedUnit {
            id: unit.pkg.package_id().to_owned(),
            target: unit.target.clone(),
            profile: unit.profile,
            kind: unit.kind,
            mode: unit.mode,
        }
    }
}

impl BuildKey for OwnedUnit {
    type Key = UnitKey;

    fn key(&self) -> UnitKey {
        (self.id.clone(), self.target.clone(), self.mode)
    }
}

impl BuildGraph for CargoPlan {
    type Unit = OwnedUnit;

    fn units(&self) -> Vec<&Self::Unit> {
        self.units.values().collect()
    }
    fn get(&self, key: <Self::Unit as BuildKey>::Key) -> Option<&Self::Unit> {
        self.units.get(&key)
    }
    fn get_mut(&mut self, key: <Self::Unit as BuildKey>::Key) -> Option<&mut Self::Unit> {
        self.units.get_mut(&key)
    }
    fn deps(&self, key: <Self::Unit as BuildKey>::Key) -> Vec<&Self::Unit> {
        self.dep_graph
            .get(&key)
            .map(|d| d.iter().map(|d| &self.units[d]).collect())
            .unwrap_or_default()
    }

    fn add<T: Into<Self::Unit>>(&mut self, unit: T, deps: Vec<T>) {
        let unit = unit.into();
        // Units can depend on others with different Targets or Profiles
        // (e.g. different `run_custom_build`) despite having the same UnitKey.
        // We coalesce them here while creating the UnitKey dep graph.
        // TODO: Are we sure? Can we figure that out?
        let deps = deps.into_iter().map(|d| d.into()).filter(|dep| unit.key() != dep.key());

        for dep in deps {
            self.dep_graph.entry(unit.key()).or_insert_with(HashSet::new).insert(dep.key());
            self.rev_dep_graph.entry(dep.key()).or_insert_with(HashSet::new).insert(unit.key());

            self.units.entry(dep.key()).or_insert(dep);
        }

        // We expect these entries to be present for each unit in the graph
        self.dep_graph.entry(unit.key()).or_insert_with(HashSet::new);
        self.rev_dep_graph.entry(unit.key()).or_insert_with(HashSet::new);

        self.units.entry(unit.key()).or_insert(unit);
    }

    fn dirties<T: AsRef<Path>>(&self, files: &[T]) -> Vec<&Self::Unit> {
        self.fetch_dirty_units(files)
            .iter()
            .map(|key| self.units.get(key).expect("dirties"))
            .collect()
    }

    fn dirties_transitive<T: AsRef<Path>>(&self, files: &[T]) -> Vec<&Self::Unit> {
        let dirties = self.fetch_dirty_units(files);

        self.transitive_dirty_units(&dirties)
            .iter()
            .map(|key| self.units.get(key).expect("dirties_transitive"))
            .collect()
    }

    fn topological_sort(&self, units: Vec<&Self::Unit>) -> Vec<&Self::Unit> {
        let keys = units.into_iter().map(|u| u.key()).collect();
        let graph = self.dirty_rev_dep_graph(&keys);

        CargoPlan::topological_sort(self, &graph)
            .iter()
            .map(|key| self.units.get(key).expect("topological_sort"))
            .collect()
    }

    fn prepare_work<T: AsRef<Path> + std::fmt::Debug>(&self, files: &[T]) -> WorkStatus {
        CargoPlan::prepare_work(self, files)
    }
}
