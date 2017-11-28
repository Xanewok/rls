// Copyright 2017 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use build::BuildResult;
use lsp_data::{ls_util, PublishDiagnosticsParams};
use actions::notifications::{DiagnosticsBegin, DiagnosticsEnd, PublishDiagnostics};
use server::{Notification, Output};
use CRATE_BLACKLIST;
use Span;

use analysis::AnalysisHost;
use data::Analysis;
use ls_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Range};
use serde_json;
use span::compiler::DiagnosticSpan;
use url::Url;


pub type BuildResults = HashMap<PathBuf, Vec<(Diagnostic, Vec<Suggestion>)>>;

pub struct PostBuildHandler<O: Output> {
    pub analysis: Arc<AnalysisHost>,
    pub previous_build_results: Arc<Mutex<BuildResults>>,
    pub project_path: PathBuf,
    pub out: O,
    pub show_warnings: bool,
    pub use_black_list: bool,
}

impl<O: Output> PostBuildHandler<O> {
    pub fn handle(self, result: BuildResult) {
        self.out.notify(Notification::<DiagnosticsBegin>::new(()));

        match result {
            BuildResult::Success(messages, new_analysis)
            | BuildResult::Failure(messages, new_analysis) => {
                thread::spawn(move || {
                    trace!("build - Success");

                    // Emit appropriate diagnostics using the ones from build.
                    self.handle_messages(messages);

                    // Reload the analysis data.
                    debug!("reload analysis: {:?}", self.project_path);
                    if new_analysis.is_empty() {
                        self.reload_analysis_from_disk();
                    } else {
                        self.reload_analysis_from_memory(new_analysis);
                    }

                    self.out.notify(Notification::<DiagnosticsEnd>::new(()));
                });
            }
            BuildResult::Squashed => {
                trace!("build - Squashed");
                self.out.notify(Notification::<DiagnosticsEnd>::new(()));
            }
            BuildResult::Err => {
                trace!("build - Error");
                self.out.notify(Notification::<DiagnosticsEnd>::new(()));
            }
        }
    }

    fn handle_messages(&self, messages: Vec<String>) {
        // These notifications will include empty sets of errors for files
        // which had errors, but now don't. This instructs the IDE to clear
        // errors for those files.
        let mut results = self.previous_build_results.lock().unwrap();
        // We must not clear the hashmap, just the values in each list.
        // This allows us to save allocated before memory.
        for v in &mut results.values_mut() {
            v.clear();
        }

        let cwd = ::std::env::current_dir().unwrap();

        for msg in &messages {
            if let Some(FileDiagnostic {
                file_path,
                diagnostic,
                suggestions,
            }) = parse_diagnostics(msg) {
                results
                    .entry(cwd.join(file_path))
                    .or_insert_with(Vec::new)
                    .push((diagnostic, suggestions));
            }
        }

        emit_notifications(&results, self.show_warnings, &self.out);
    }

    fn reload_analysis_from_disk(&self) {
        let cwd = ::std::env::current_dir().unwrap();
        if self.use_black_list {
            self.analysis
                .reload_with_blacklist(&self.project_path, &cwd, &CRATE_BLACKLIST)
                .unwrap();
        } else {
            self.analysis.reload(&self.project_path, &cwd).unwrap();
        }
    }

    fn reload_analysis_from_memory(&self, analysis: Vec<Analysis>) {
        let cwd = ::std::env::current_dir().unwrap();
        if self.use_black_list {
            self.analysis
                .reload_from_analysis(analysis, &self.project_path, &cwd, &CRATE_BLACKLIST)
                .unwrap();
        } else {
            self.analysis
                .reload_from_analysis(analysis, &self.project_path, &cwd, &[])
                .unwrap();
        }
    }
}

#[derive(Debug)]
pub struct Suggestion {
    pub range: Range,
    pub new_text: String,
    pub label: String,
}

#[derive(Debug)]
struct FileDiagnostic {
    file_path: PathBuf,
    diagnostic: Diagnostic,
    suggestions: Vec<Suggestion>,
}

#[derive(Debug, Deserialize)]
struct CompilerMessage {
    message: String,
    code: Option<CompilerMessageCode>,
    level: String,
    spans: Vec<DiagnosticSpan>,
    children: Vec<CompilerMessage>,
}

#[derive(Debug, Deserialize)]
struct CompilerMessageCode {
    code: String,
}

fn parse_diagnostics(message: &str) -> Option<FileDiagnostic> {
    let message = match serde_json::from_str::<CompilerMessage>(message) {
        Ok(m) => m,
        Err(e) => {
            debug!("build error {:?}", e);
            debug!("from {}", message);
            return None;
        }
    };

    if message.spans.is_empty() {
        return None;
    }

    let primary_span = primary_span(&message);
    let suggestions = make_suggestions(message.children, &primary_span.file);

    let diagnostic = Diagnostic {
        range: ls_util::rls_to_range(primary_span.range),
        severity: Some(severity(&message.level)),
        code: Some(NumberOrString::String(match message.code {
            Some(c) => c.code.clone(),
            None => String::new(),
        })),
        source: Some("rustc".into()),
        message: message.message,
    };

    Some(FileDiagnostic {
        file_path: primary_span.file,
        diagnostic: diagnostic,
        suggestions: suggestions,
    })
}

fn severity(level: &str) -> DiagnosticSeverity {
    if level == "error" {
        DiagnosticSeverity::Error
    } else {
        DiagnosticSeverity::Warning
    }
}

fn make_suggestions(children: Vec<CompilerMessage>, file: &Path) -> Vec<Suggestion> {
    let mut suggestions = vec![];
    for c in children {
        for sp in c.spans {
            let span = sp.rls_span().zero_indexed();
            if span.file == file {
                if let Some(s) = sp.suggested_replacement {
                    let suggestion = Suggestion {
                        new_text: s.clone(),
                        range: ls_util::rls_to_range(span.range),
                        label: format!("{}: `{}`", c.message, s),
                    };
                    suggestions.push(suggestion);
                }
            }
        }
    }
    suggestions
}

fn primary_span(message: &CompilerMessage) -> Span {
    let primary = message
        .spans
        .iter()
        .filter(|x| x.is_primary)
        .next()
        .unwrap()
        .clone();
    primary.rls_span().zero_indexed()
}

fn emit_notifications<O: Output>(build_results: &BuildResults, show_warnings: bool, out: &O) {
    for (path, diagnostics) in build_results {
        let params = PublishDiagnosticsParams {
            uri: Url::from_file_path(path).unwrap(),
            diagnostics: diagnostics
                .iter()
                .filter_map(|&(ref d, _)| {
                    if show_warnings || d.severity != Some(DiagnosticSeverity::Warning) {
                        Some(d.clone())
                    } else {
                        None
                    }
                })
                .collect(),
        };

        out.notify(Notification::<PublishDiagnostics>::new(params));
    }
}
