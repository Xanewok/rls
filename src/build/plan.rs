use cargo::util::{process, ProcessBuilder};
use serde_derive::Deserialize;

use std::collections::HashMap;
use std::path::PathBuf;

use super::cargo_plan::WorkStatus;

// Primitives:
// Invocation dep graph
//

#[derive(Debug, Deserialize)]
struct RawPlan {
    invocations: Vec<RawInvocation>,
}

#[derive(Debug, Deserialize)]
struct RawInvocation {
    deps: Vec<usize>,
    outputs: Vec<PathBuf>,
    links: HashMap<PathBuf, PathBuf>,
    program: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    cwd: Option<PathBuf>,
}

/// Safe build plan type, invocation dependencies are guaranteed to be inside
/// the plan.
crate struct BuildPlan {
    invocations: Vec<Invocation>,
}

crate struct Invocation {
    deps: Vec<usize>, // FIXME: Use arena and store refs instead for ergonomics
    outputs: Vec<PathBuf>,
    links: HashMap<PathBuf, PathBuf>,
    command: ProcessBuilder,
}

impl Invocation {
    fn deps<'a>(&'a self, plan: &'a BuildPlan) -> impl Iterator<Item = &'a Invocation> {
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
            command,
        }
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

        Ok(BuildPlan {
            invocations: raw.invocations.into_iter().map(|x| x.into()).collect(),
        })
    }
}
