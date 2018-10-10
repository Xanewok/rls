#![feature(tool_lints)]

// Copyright 2016 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#[macro_use]
extern crate serde_json;

mod support;

use self::support::{basic_bin_manifest, project};
use crate::support::RlsStdout;
use std::io::Write;
use std::time::Duration;

/// Returns a timeout for waiting for rls stdout messages
///
/// Env var `RLS_TEST_WAIT_FOR_AGES` allows super long waiting for CI
fn rls_timeout() -> Duration {
    Duration::from_secs(if std::env::var("RLS_TEST_WAIT_FOR_AGES").is_ok() {
        300
    } else {
        15
    })
}

fn rfind_diagnostics_with_uri(stdout: &RlsStdout, uri_end: &str) -> serde_json::Value {
    stdout
        .to_json_messages()
        .filter(|json| json["method"] == "textDocument/publishDiagnostics")
        .rfind(|json| json["params"]["uri"].as_str().unwrap().ends_with(uri_end))
        .unwrap()
}

#[test]
fn cmd_test_infer_bin() {
    let p = project("simple_workspace")
        .file("Cargo.toml", &basic_bin_manifest("foo"))
        .file(
            "src/main.rs",
            r#"
                struct UnusedBin;
                fn main() {
                    println!("Hello world!");
                }
            "#,
        )
        .build();

    let root_path = p.root();
    let mut rls = p.spawn_rls();

    rls.request(
        0,
        "initialize",
        Some(json!({
            "rootPath": root_path,
            "capabilities": {}
        })),
    )
    .unwrap();

    let json: Vec<_> = rls
        .wait_until_done_indexing(rls_timeout())
        .to_json_messages()
        .filter(|json| json["method"] != "window/progress")
        .collect();

    assert!(json.len() > 1);

    assert!(json[0]["result"]["capabilities"].is_object());

    assert_eq!(json[1]["method"], "textDocument/publishDiagnostics");
    assert_eq!(json[1]["params"]["diagnostics"][0]["code"], "dead_code");

    rls.shutdown(rls_timeout());
}

/// Test includes window/progress regression testing
#[test]
fn cmd_test_simple_workspace() {
    let p = project("simple_workspace")
        .file(
            "Cargo.toml",
            r#"
                [workspace]
                members = [
                "member_lib",
                "member_bin",
                ]
            "#,
        )
        .file(
            "Cargo.lock",
            r#"
                [root]
                name = "member_lib"
                version = "0.1.0"

                [[package]]
                name = "member_bin"
                version = "0.1.0"
                dependencies = [
                "member_lib 0.1.0",
                ]
            "#,
        )
        .file(
            "member_bin/Cargo.toml",
            r#"
                [package]
                name = "member_bin"
                version = "0.1.0"
                authors = ["Igor Matuszewski <Xanewok@gmail.com>"]

                [dependencies]
                member_lib = { path = "../member_lib" }
            "#,
        )
        .file(
            "member_bin/src/main.rs",
            r#"
                extern crate member_lib;

                fn main() {
                    let a = member_lib::MemberLibStruct;
                }
            "#,
        )
        .file(
            "member_lib/Cargo.toml",
            r#"
                [package]
                name = "member_lib"
                version = "0.1.0"
                authors = ["Igor Matuszewski <Xanewok@gmail.com>"]

                [dependencies]
            "#,
        )
        .file(
            "member_lib/src/lib.rs",
            r#"
                pub struct MemberLibStruct;

                struct Unused;

                #[cfg(test)]
                mod tests {
                    #[test]
                    fn it_works() {
                    }
                }
            "#,
        )
        .build();

    let root_path = p.root();
    let mut rls = p.spawn_rls();

    rls.request(
        0,
        "initialize",
        Some(json!({
            "rootPath": root_path,
            "capabilities": {}
        })),
    )
    .unwrap();

    let json: Vec<_> = rls
        .wait_until_done_indexing(rls_timeout())
        .to_json_messages()
        .collect();
    assert!(json.len() >= 11);

    assert!(json[0]["result"]["capabilities"].is_object());

    assert_eq!(json[1]["method"], "window/progress");
    assert_eq!(json[1]["params"]["title"], "Building");
    assert_eq!(json[1]["params"].get("message"), None);

    // order of member_lib/member_bin is undefined
    for json in &json[2..6] {
        assert_eq!(json["method"], "window/progress");
        assert_eq!(json["params"]["title"], "Building");
        assert!(
            json["params"]["message"]
                .as_str()
                .unwrap()
                .starts_with("member_")
        );
    }

    assert_eq!(json[6]["method"], "window/progress");
    assert_eq!(json[6]["params"]["done"], true);
    assert_eq!(json[6]["params"]["title"], "Building");

    assert_eq!(json[7]["method"], "window/progress");
    assert_eq!(json[7]["params"]["title"], "Indexing");

    assert_eq!(json[8]["method"], "textDocument/publishDiagnostics");

    assert_eq!(json[9]["method"], "textDocument/publishDiagnostics");

    assert_eq!(json[10]["method"], "window/progress");
    assert_eq!(json[10]["params"]["done"], true);
    assert_eq!(json[10]["params"]["title"], "Indexing");

    let json = rls
        .shutdown(rls_timeout())
        .to_json_messages()
        .nth(11)
        .expect("No shutdown response received");

    assert_eq!(json["id"], 99999);
}

#[test]
fn cmd_changing_workspace_lib_retains_bin_diagnostics() {
    let p = project("simple_workspace")
        .file(
            "Cargo.toml",
            r#"
                [workspace]
                members = [
                "library",
                "binary",
                ]
            "#,
        )
        .file(
            "library/Cargo.toml",
            r#"
                [package]
                name = "library"
                version = "0.1.0"
                authors = ["Example <rls@example.com>"]
            "#,
        )
        .file(
            "library/src/lib.rs",
            r#"
                pub fn fetch_u32() -> u32 {
                    let unused = ();
                    42
                }
                #[cfg(test)]
                mod test {
                    #[test]
                    fn my_test() {
                        let test_val: u32 = super::fetch_u32();
                    }
                }
            "#,
        )
        .file(
            "binary/Cargo.toml",
            r#"
                [package]
                name = "binary"
                version = "0.1.0"
                authors = ["Igor Matuszewski <Xanewok@gmail.com>"]

                [dependencies]
                library = { path = "../library" }
            "#,
        )
        .file(
            "binary/src/main.rs",
            r#"
                extern crate library;

                fn main() {
                    let val: u32 = library::fetch_u32();
                }
            "#,
        )
        .build();

    let root_path = p.root();
    let mut rls = p.spawn_rls();

    rls.request(
        0,
        "initialize",
        Some(json!({
            "rootPath": root_path,
            "capabilities": {}
        })),
    )
    .unwrap();

    let stdout = rls.wait_until_done_indexing(rls_timeout());

    let lib_diagnostic = rfind_diagnostics_with_uri(&stdout, "library/src/lib.rs");
    assert_eq!(
        lib_diagnostic["params"]["diagnostics"][0]["code"],
        "unused_variables"
    );
    let bin_diagnostic = rfind_diagnostics_with_uri(&stdout, "binary/src/main.rs");
    assert_eq!(
        bin_diagnostic["params"]["diagnostics"][0]["code"],
        "unused_variables"
    );

    rls.notify(
        "textDocument/didChange",
        Some(json!({
                "contentChanges": [
                    {
                        "range": {
                            "start": {
                                "line": 1,
                                "character": 38,
                            },
                            "end": {
                                "line": 1,
                                "character": 41,
                            }
                        },
                        "rangeLength": 3,
                        "text": "u64"
                    }
                ],
                "textDocument": {
                    "uri": format!("file://{}/library/src/lib.rs", root_path.as_path().display()),
                    "version": 0
                }
            })),
    )
    .unwrap();

    let stdout = rls.wait_until_done_indexing_n(2, rls_timeout());

    // lib unit tests have compile errors
    let lib_diagnostic = rfind_diagnostics_with_uri(&stdout, "library/src/lib.rs");
    let error_diagnostic = lib_diagnostic["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "E0308")
        .expect("expected lib error diagnostic");
    assert!(
        error_diagnostic["message"]
            .as_str()
            .unwrap()
            .contains("expected u32, found u64")
    );

    // bin depending on lib picks up type mismatch
    let bin_diagnostic = rfind_diagnostics_with_uri(&stdout, "binary/src/main.rs");
    let error_diagnostic = bin_diagnostic["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "E0308")
        .expect("expected bin error diagnostic");
    assert!(
        error_diagnostic["message"]
            .as_str()
            .unwrap()
            .contains("expected u32, found u64")
    );

    rls.notify(
        "textDocument/didChange",
        Some(json!({
                "contentChanges": [
                    {
                        "range": {
                            "start": {
                                "line": 1,
                                "character": 38,
                            },
                            "end": {
                                "line": 1,
                                "character": 41,
                            }
                        },
                        "rangeLength": 3,
                        "text": "u32"
                    }
                ],
                "textDocument": {
                    "uri": format!("file://{}/library/src/lib.rs", root_path.as_path().display()),
                    "version": 1
                }
            })),
    )
    .unwrap();

    let stdout = rls.wait_until_done_indexing_n(3, rls_timeout());
    let lib_diagnostic = rfind_diagnostics_with_uri(&stdout, "library/src/lib.rs");
    assert_eq!(
        lib_diagnostic["params"]["diagnostics"][0]["code"],
        "unused_variables"
    );
    let bin_diagnostic = rfind_diagnostics_with_uri(&stdout, "binary/src/main.rs");
    assert_eq!(
        bin_diagnostic["params"]["diagnostics"][0]["code"],
        "unused_variables"
    );

    rls.shutdown(rls_timeout());
}

/// Tests whether a project with a Cargo build script is compiled correctly and
/// that modifying it triggers a project rebuild.
#[test]
fn modified_build_script_rebuilds_project() {
    let p = project("simple_workspace")
        .file(
            "Cargo.toml",
            r#"
                [workspace]
                members = [
                "crate_a",
                "crate_b",
                ]
            "#,
        )
        .file(
            "crate_a/Cargo.toml",
            r#"
                [package]
                name = "crate_a"
                version = "0.1.0"
                authors = ["Igor Matuszewski <Xanewok@gmail.com>"]
            "#,
        )
        .file(
            "crate_a/build.rs",
            r#"
                use std::io::Write;

                fn main() {
                    let output = std::process::Command::new("echo").args(&["mystring"]).output().unwrap();

                    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
                    let mut f = std::fs::File::create(out_dir.join("file.txt")).unwrap();
                    f.write_all(&output.stdout).unwrap();
                }

            "#,
        )
        .file(
            "crate_a/src/lib.rs",
            r#"
                const BUILT_STRING: &'static str = include_str!(concat!(env!("OUT_DIR"), "/file.txt"));

                struct StructA;
            "#,
        )
        .file(
            "crate_b/Cargo.toml",
            r#"
                [package]
                name = "crate_b"
                version = "0.1.0"
                authors = ["Igor Matuszewski <Xanewok@gmail.com>"]

                [dependencies]
                crate_a = { path = "../crate_a" }
            "#,
        )
        .file(
            "crate_b/src/main.rs",
            r#"

                struct StructB;
            "#,
        )
        .build();

    let root_path = p.root();
    let mut rls = p.spawn_rls();

    rls.request(
        0,
        "initialize",
        Some(json!({
            "rootPath": root_path,
            "capabilities": {}
        })),
    )
    .unwrap();

    let stdout = rls.wait_until_done_indexing(rls_timeout());
    let json: Vec<_> = stdout.to_json_messages().collect();
    assert!(json.len() >= 11);

    assert!(json[0]["result"]["capabilities"].is_object());

    assert_eq!(json[1]["method"], "window/progress");
    assert_eq!(json[1]["params"]["title"], "Building");
    assert_eq!(json[1]["params"].get("message"), None);

    let crates = ["build_script_build", "crate_a", "crate_a", "crate_b", "crate_b"];
    for (json, &crate_name) in json[2..crates.len()].iter().zip(crates.iter()) {
        assert_eq!(json["method"], "window/progress");
        assert_eq!(json["params"]["title"], "Building");
        assert!(
            json["params"]["message"]
                .as_str()
                .unwrap()
                .starts_with(crate_name)
        );
    }

    assert_eq!(json[7]["method"], "window/progress");
    assert_eq!(json[7]["params"]["done"], true);
    assert_eq!(json[7]["params"]["title"], "Building");

    assert_eq!(json[8]["method"], "window/progress");
    assert_eq!(json[8]["params"]["title"], "Indexing");

    assert_eq!(json[9]["method"], "textDocument/publishDiagnostics");
    assert_eq!(json[10]["method"], "textDocument/publishDiagnostics");

    for msg in &[
        "constant item is never used: `BUILT_STRING`",
        "struct is never constructed: `StructB`",
    ] {
        &json[9..11]
            .iter()
            .flat_map(|json| json["params"]["diagnostics"].as_array().unwrap())
            .any(|diag| diag["message"].as_str().unwrap().contains(msg));
    }

    assert_eq!(json[11]["method"], "window/progress");
    assert_eq!(json[11]["params"]["done"], true);
    assert_eq!(json[11]["params"]["title"], "Indexing");

    rls.notify(
        "textDocument/didChange",
        Some(json!({
                "contentChanges": [
                    {
                        "range": {
                            "start": { "line": 1, "character": 1 },
                            "end": { "line": 1, "character": 1 }
                        },
                        "rangeLength": 0,
                        "text": ""
                    }
                ],
                "textDocument": {
                    "uri": format!("file://{}/crate_b/src/main.rs", root_path.as_path().display()),
                    "version": 1
                }
            })),
    )
    .unwrap();

    let stdout = rls.wait_until_done_indexing_n(2, rls_timeout());

    // Unfortunately we do what Cargo does - we display diagnostics only for
    // rebuilt dirty work (which is only crate_b in this case)
    let a_diagnostic = rfind_diagnostics_with_uri(&stdout, "crate_a/src/lib.rs");
    assert_eq!(a_diagnostic["params"]["diagnostics"], json!([]));
    let b_diagnostic = rfind_diagnostics_with_uri(&stdout, "crate_b/src/main.rs");
    assert!(
        b_diagnostic["params"]["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("struct is never constructed: `StructB`")
    );

    // Touch build script, triggering project rebuild
    rls.notify(
        "textDocument/didChange",
        Some(json!({
                "contentChanges": [
                    {
                        "range": {
                            "start": { "line": 1, "character": 1 },
                            "end": { "line": 1, "character": 1 }
                        },
                        "rangeLength": 0,
                        "text": ""
                    }
                ],
                "textDocument": {
                    "uri": format!("file://{}/crate_a/build.rs", root_path.as_path().display()),
                    "version": 1
                }
            })),
    )
    .unwrap();

    let stdout = rls.wait_until_done_indexing_n(3, rls_timeout());
    let json: Vec<_> = stdout.to_json_messages().collect();

    assert_eq!(json[20]["method"], "window/progress");
    assert_eq!(json[20]["params"]["title"], "Building");
    assert_eq!(json[20]["params"].get("message"), None);

    // Every crate transitively depending on the build script gets rebuilt
    for (json, &crate_name) in json[21..21 + crates.len()].iter().zip(crates.iter()) {
        assert_eq!(json["method"], "window/progress");
        assert_eq!(json["params"]["title"], "Building");
        assert!(
            json["params"]["message"]
                .as_str()
                .unwrap()
                .starts_with(crate_name)
        );
    }

    assert_eq!(json[26]["method"], "window/progress");
    assert_eq!(json[26]["params"]["done"], true);
    assert_eq!(json[26]["params"]["title"], "Building");

    assert_eq!(json[27]["method"], "window/progress");
    assert_eq!(json[27]["params"]["title"], "Indexing");

    assert_eq!(json[28]["method"], "textDocument/publishDiagnostics");
    assert_eq!(json[29]["method"], "textDocument/publishDiagnostics");

    for msg in &[
        "constant item is never used: `BUILT_STRING`",
        "struct is never constructed: `StructB`",
    ] {
        &json[28..30]
            .iter()
            .flat_map(|json| json["params"]["diagnostics"].as_array().unwrap())
            .any(|diag| diag["message"].as_str().unwrap().contains(msg));
    }

    assert_eq!(json[30]["method"], "window/progress");
    assert_eq!(json[30]["params"]["done"], true);
    assert_eq!(json[30]["params"]["title"], "Indexing");

    let json = rls
        .shutdown(rls_timeout())
        .to_json_messages()
        .nth(31)
        .expect("No shutdown response received");

    assert_eq!(json["id"], 99999);
}

#[test]
fn cmd_test_complete_self_crate_name() {
    let p = project("ws_with_test_dir")
        .file(
            "Cargo.toml",
            r#"
                [workspace]
                members = ["library"]
            "#,
        )
        .file(
            "library/Cargo.toml",
            r#"
                [package]
                name = "library"
                version = "0.1.0"
                authors = ["Example <rls@example.com>"]
            "#,
        )
        .file(
            "library/src/lib.rs",
            r#"
                pub fn function() -> usize { 5 }
            "#,
        )
        .file(
            "library/tests/test.rs",
            r#"
                   extern crate library;
                   use library::~
            "#,
        )
        .build();

    let root_path = p.root();
    let mut rls = p.spawn_rls();

    rls.request(
        0,
        "initialize",
        Some(json!({
            "rootPath": root_path,
            "capabilities": {}
        })),
    )
    .unwrap();

    let stdout = rls.wait_until_done_indexing(rls_timeout());

    let json: Vec<_> = stdout
        .to_json_messages()
        .filter(|json| json["method"] != "window/progress")
        .collect();
    assert!(json.len() > 1);

    assert!(json[0]["result"]["capabilities"].is_object());

    assert_eq!(json[1]["method"], "textDocument/publishDiagnostics");
    assert!(
        json[1]["params"]["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("expected identifier")
    );

    rls.request(
        0,
        "textDocument/completion",
        Some(json!({
            "context": {
                "triggerCharacter": ":",
                "triggerKind": 2
            },
            "position": {
                "character": 32,
                "line": 2
            },
            "textDocument": {
                "uri": format!("file://{}/library/tests/test.rs", root_path.as_path().display()),
                "version": 1
            }
        })),
    )
    .unwrap();

    let stdout = rls.wait_until(
        |stdout| {
            stdout
                .to_json_messages()
                .any(|json| json["result"][0]["detail"].is_string())
        },
        rls_timeout(),
    );

    let json = stdout
        .to_json_messages()
        .rfind(|json| json["result"].is_array())
        .unwrap();

    assert_eq!(json["result"][0]["detail"], "pub fn function() -> usize");

    rls.shutdown(rls_timeout());
}

#[test]
fn test_completion_suggests_arguments_in_statements() {
    let p = project("ws_with_test_dir")
        .file(
            "Cargo.toml",
            r#"
                [workspace]
                members = ["library"]
            "#,
        ).file(
            "library/Cargo.toml",
            r#"
                [package]
                name = "library"
                version = "0.1.0"
                authors = ["Example <rls@example.com>"]
            "#,
        ).file(
            "library/src/lib.rs",
            r#"
                pub fn function() -> usize { 5 }
            "#,
        ).file(
            "library/tests/test.rs",
            r#"
                   extern crate library;
                   fn magic() {
                       let a = library::f~
                   }
            "#,
        ).build();

    let root_path = p.root();
    let mut rls = p.spawn_rls();

    rls.request(
        0,
        "initialize",
        Some(json!({
            "rootPath": root_path,
            "capabilities": {
                "textDocument": {
                    "completion": {
                        "completionItem": {
                            "snippetSupport": true
                        }
                    }
                }
            }
        })),
    ).unwrap();

    rls.request(
        0,
        "textDocument/completion",
        Some(json!({
            "context": {
                "triggerCharacter": "f",
                "triggerKind": 2
            },
            "position": {
                "character": 41,
                "line": 3
            },
            "textDocument": {
                "uri": format!("file://{}/library/tests/test.rs", root_path.as_path().display()),
                "version": 1
            }
        })),
    ).unwrap();

    let stdout = rls.wait_until(
        |stdout| {
            stdout
                .to_json_messages()
                .any(|json| json["result"][0]["detail"].is_string())
        },
        rls_timeout(),
    );
    let json = stdout
        .to_json_messages()
        .rfind(|json| json["result"].is_array())
        .unwrap();

    assert_eq!(json["result"][0]["insertText"], "function()");

    rls.shutdown(rls_timeout());
}

#[test]
fn test_use_statement_completion_doesnt_suggest_arguments() {
    let p = project("ws_with_test_dir")
        .file(
            "Cargo.toml",
            r#"
                [workspace]
                members = ["library"]
            "#,
        ).file(
            "library/Cargo.toml",
            r#"
                [package]
                name = "library"
                version = "0.1.0"
                authors = ["Example <rls@example.com>"]
            "#,
        ).file(
            "library/src/lib.rs",
            r#"
                pub fn function() -> usize { 5 }
            "#,
        ).file(
            "library/tests/test.rs",
            r#"
                   extern crate library;
                   use library::~;
            "#,
        ).build();

    //32, 2
    let root_path = p.root();
    let mut rls = p.spawn_rls();

    rls.request(
        0,
        "initialize",
        Some(json!({
            "rootPath": root_path,
            "capabilities": {}
        })),
    ).unwrap();

    rls.request(
        0,
        "textDocument/completion",
        Some(json!({
            "context": {
                "triggerCharacter": ":",
                "triggerKind": 2
            },
            "position": {
                "character": 32,
                "line": 2
            },
            "textDocument": {
                "uri": format!("file://{}/library/tests/test.rs", root_path.as_path().display()),
                "version": 1
            }
        })),
    ).unwrap();

    let stdout = rls.wait_until(
        |stdout| {
            stdout
                .to_json_messages()
                .any(|json| json["result"][0]["detail"].is_string())
        },
        rls_timeout(),
    );
    let json = stdout
        .to_json_messages()
        .rfind(|json| json["result"].is_array())
        .unwrap();

    assert_eq!(json["result"][0]["insertText"], "function");

    rls.shutdown(rls_timeout());
}

/// Test simulates typing in a dependency wrongly in a couple of ways before finally getting it
/// right. Rls should provide Cargo.toml diagnostics.
///
/// ```
/// [dependencies]
/// version-check = "0.5555"
/// ```
///
/// * Firstly "version-check" doesn't exist, it should be "version_check"
/// * Secondly version 0.5555 of "version_check" doesn't exist.
#[test]
fn cmd_dependency_typo_and_fix() {
    let manifest_with_dependency = |dep: &str| {
        format!(
            r#"
            [package]
            name = "dependency_typo"
            version = "0.1.0"
            authors = ["alexheretic@gmail.com"]

            [dependencies]
            {}
        "#,
            dep
        )
    };

    let project = project("dependency_typo")
        .file(
            "Cargo.toml",
            &manifest_with_dependency(r#"version-check = "0.5555""#),
        )
        .file(
            "src/main.rs",
            r#"
                fn main() {
                    println!("Hello world!");
                }
            "#,
        )
        .build();
    let root_path = project.root();
    let mut rls = project.spawn_rls();

    rls.request(
        0,
        "initialize",
        Some(json!({
            "rootPath": root_path,
            "capabilities": {}
        })),
    )
    .unwrap();

    let publish = rls
        .wait_until_done_indexing(rls_timeout())
        .to_json_messages()
        .rfind(|m| m["method"] == "textDocument/publishDiagnostics")
        .expect("No publishDiagnostics");

    let diags = &publish["params"]["diagnostics"];
    assert_eq!(diags.as_array().unwrap().len(), 1);
    assert!(
        diags[0]["message"]
            .as_str()
            .unwrap()
            .contains("no matching package named `version-check`")
    );
    assert_eq!(diags[0]["severity"], 1);

    let change_manifest = |contents: &str| {
        let mut manifest = std::fs::OpenOptions::new()
            .write(true)
            .open(root_path.join("Cargo.toml"))
            .unwrap();

        manifest.set_len(0).unwrap();
        write!(manifest, "{}", contents,).unwrap();
    };

    // fix naming typo, we now expect a version error diagnostic
    change_manifest(&manifest_with_dependency(
        r#"version_check = "0.5555""#,
    ));
    rls.request(
        1,
        "workspace/didChangeWatchedFiles",
        Some(json!({
            "changes": [{
                "uri": format!("file://{}/Cargo.toml", root_path.as_path().display()),
                "type": 2
            }],
        })),
    )
    .unwrap();

    let publish = rls
        .wait_until_done_indexing_n(2, rls_timeout())
        .to_json_messages()
        .rfind(|m| m["method"] == "textDocument/publishDiagnostics")
        .expect("No publishDiagnostics");

    let diags = &publish["params"]["diagnostics"];
    assert_eq!(diags.as_array().unwrap().len(), 1);
    assert!(
        diags[0]["message"]
            .as_str()
            .unwrap()
            .contains("^0.5555")
    );
    assert_eq!(diags[0]["severity"], 1);

    // Fix version issue so no error diagnostics occur.
    // This is kinda slow as cargo will compile the dependency, though I
    // chose version_check to minimise this as it is a very small dependency.
    change_manifest(&manifest_with_dependency(r#"version_check = "0.1""#));
    rls.request(
        2,
        "workspace/didChangeWatchedFiles",
        Some(json!({
            "changes": [{
                "uri": format!("file://{}/Cargo.toml", root_path.as_path().display()),
                "type": 2
            }],
        })),
    )
    .unwrap();

    let publish = rls
        .wait_until_done_indexing_n(3, rls_timeout())
        .to_json_messages()
        .rfind(|m| m["method"] == "textDocument/publishDiagnostics")
        .expect("No publishDiagnostics");

    let diags = &publish["params"]["diagnostics"];

    assert_eq!(
        diags
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["severity"] == 1),
        None
    );

    rls.shutdown(rls_timeout());
}

/// Tests correct positioning of a toml parse error, use of `==` instead of `=`.
#[test]
fn cmd_invalid_toml_manifest() {
    let project = project("invalid_toml")
        .file(
            "Cargo.toml",
            r#"[package]
            name = "probably_valid"
            version == "0.1.0"
            authors = ["alexheretic@gmail.com"]
            "#,
        )
        .file(
            "src/main.rs",
            r#"
                fn main() {
                    println!("Hello world!");
                }
            "#,
        )
        .build();
    let root_path = project.root();
    let mut rls = project.spawn_rls();

    rls.request(
        0,
        "initialize",
        Some(json!({
            "rootPath": root_path,
            "capabilities": {}
        })),
    )
    .unwrap();

    let publish = rls
        .wait_until_done_indexing(rls_timeout())
        .to_json_messages()
        .rfind(|m| m["method"] == "textDocument/publishDiagnostics")
        .expect("No publishDiagnostics");

    let diags = &publish["params"]["diagnostics"];
    assert_eq!(diags.as_array().unwrap().len(), 1);
    assert_eq!(diags[0]["severity"], 1);
    assert!(
        diags[0]["message"]
            .as_str()
            .unwrap()
            .contains("failed to parse manifest")
    );
    assert_eq!(
        diags[0]["range"],
        json!({ "start": { "line": 2, "character": 21 }, "end": { "line": 2, "character": 22 }})
    );

    rls.shutdown(rls_timeout());
}
