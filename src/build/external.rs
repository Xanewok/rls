// Copyright 2018 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Performs a build using a provided black-box build command, which ought to
//! return a list of save-analysis JSON files to be reloaded by the RLS.
//! Please note that since the command is ran externally (at a file/OS level)
//! this doesn't work with files that are not saved.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufRead;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::BuildResult;
use super::plan::{BuildPlan, RawInvocation, RawPlan};

use log::trace;
use rls_data::{Analysis, CompilationOptions};

fn cmd_line_to_command<S: AsRef<str>>(cmd_line: &S, cwd: &Path) -> Result<Command, ()> {
    let cmd_line = cmd_line.as_ref();
    let (cmd, args) = {
        let mut words = cmd_line.split_whitespace();
        let cmd = words.next().ok_or(())?;
        (cmd, words)
    };

    let mut cmd = Command::new(cmd);
    cmd.args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(cmd)
}

/// Performs a build using an external command and interprets the results.
/// The command should output on stdout a list of save-analysis .json files
/// to be reloaded by the RLS.
/// Note: This is *very* experimental and preliminary - this can viewed as
/// an experimentation until a more complete solution emerges.
pub(super) fn build_with_external_cmd<S: AsRef<str>>(
    cmd_line: S,
    build_dir: PathBuf,
) -> (BuildResult, Result<BuildPlan, ()>) {
    let cmd_line = cmd_line.as_ref();

    let mut cmd = match cmd_line_to_command(&cmd_line, &build_dir) {
        Ok(cmd) => cmd,
        Err(_) => {
            let err_msg = format!("Couldn't treat {} as command", cmd_line);
            return (BuildResult::Err(err_msg, Some(cmd_line.to_owned())), Err(()));
        }
    };

    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(io) => {
            let err_msg = format!("Couldn't execute: {} ({:?})", cmd_line, io.kind());
            return (BuildResult::Err(err_msg, Some(cmd_line.to_owned())), Err(()));
        }
    };

    let reader = std::io::BufReader::new(child.stdout.unwrap());

    let files = reader.lines().filter_map(|res| res.ok())
        .map(PathBuf::from)
        // Relative paths are relative to build command, not RLS itself (cwd may be different)
        .map(|path| if !path.is_absolute() { build_dir.join(path) } else { path });

    let analyses = match read_analysis_files(files) {
        Ok(analyses) => analyses,
        Err(cause) => {
            let err_msg = format!("Couldn't read analysis data: {}", cause);
            return (BuildResult::Err(err_msg, Some(cmd_line.to_owned())), Err(()));
        }
    };

    let plan = plan_from_analysis(&analyses, &build_dir);
    (BuildResult::Success(build_dir, vec![], analyses, false), plan)
}

/// Reads and deserializes given save-analysis JSON files into corresponding
/// `rls_data::Analysis` for each file. If an error is encountered, a `String`
/// with the error message is returned.
fn read_analysis_files<I>(files: I) -> Result<Vec<Analysis>, String>
where
    I: Iterator,
    I::Item: AsRef<Path>,
{
    let mut analyses = Vec::new();

    for path in files {
        trace!(
            "external::read_analysis_files: Attempt to read `{}`",
            path.as_ref().display()
        );

        let mut file = File::open(path).map_err(|e| e.to_string())?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .map_err(|e| e.to_string())?;

        let data = rustc_serialize::json::decode(&contents).map_err(|e| e.to_string())?;
        analyses.push(data);
    }

    Ok(analyses)
}

fn plan_from_analysis(analysis: &[Analysis], build_dir: &Path) -> Result<BuildPlan, ()> {
    let indices: HashMap<_, usize> = analysis
        .iter()
        .enumerate()
        .map(|(idx, a)| (a.prelude.as_ref().unwrap().crate_id.disambiguator, idx))
        .collect();

    let invocations: Vec<RawInvocation> = analysis.into_iter()
        .map(|a| {
            let CompilationOptions { ref directory, ref program, ref arguments, ref output } =
                a.compilation.as_ref().unwrap();

            let deps: Vec<usize> = a.prelude.as_ref().unwrap()
                .external_crates
                .iter()
                .filter_map(|c| indices.get(&c.id.disambiguator))
                .cloned()
                .collect();

            let cwd = match directory.is_relative() {
                true => build_dir.join(directory),
                false => directory.to_owned(),
            };

            Ok(RawInvocation {
                deps,
                outputs: vec![output.clone()],
                program: program.clone(),
                args: arguments.clone(),
                env: Default::default(),
                links: Default::default(),
                cwd: Some(cwd)

            })
        })
        .collect::<Result<Vec<RawInvocation>, ()>>()?;

    BuildPlan::try_from_raw(RawPlan { invocations })
}

crate fn fetch_build_plan<S: AsRef<str>>(cmd_line: S, build_dir: PathBuf) -> Result<BuildPlan, ()> {
    let cmd_line = cmd_line.as_ref();

    let mut cmd = cmd_line_to_command(&cmd_line, &build_dir)?;
    let child = cmd.spawn().map_err(|_| ())?;

    let stdout = {
        let mut stdout = child.stdout.ok_or(())?;
        let mut buf = vec![];
        stdout.read_to_end(&mut buf).map_err(|_| ())?;
        String::from_utf8(buf).map_err(|_| ())?
    };

    let plan = serde_json::from_str::<RawPlan>(&stdout).unwrap();

    BuildPlan::try_from_raw(plan)
}
