#![feature(rustc_private)]

extern crate env_logger;
extern crate rustc;
extern crate rustc_driver;
extern crate rustc_interface;
extern crate rustc_plugin;
extern crate rustc_save_analysis;
extern crate syntax;

use rustc::session::config::{ErrorOutputType, Input};
use rustc::session::{early_error, Session};
use rustc_driver::{run_compiler, Callbacks, Compilation};
use rustc_interface::interface;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::{env, process};

use futures::future::Future;
use rls_ipc::rpc::{Crate, Edition};

mod clippy;
#[cfg(feature = "ipc")]
mod ipc;

pub fn run() -> Result<(), ()> {
    #[cfg(feature = "ipc")]
    let mut rt = tokio::runtime::Runtime::new().unwrap();

    let clippy_preference = clippy::preference();

    #[cfg(feature = "ipc")]
    let (mut shim_calls, file_loader) = match std::env::var("RLS_IPC_ENDPOINT").ok() {
        Some(endpoint) => {
            #[allow(deprecated)] // Windows doesn't work with lazily-bound reactors
            let reactor = rt.reactor().clone();
            let connection =
                ipc::connect(endpoint, &reactor).expect("Couldn't connect to IPC endpoint");
            let client: ipc::Client =
                rt.block_on(connection).expect("Couldn't connect to IPC endpoint");
            let (file_loader, callbacks) = client.split();

            (ShimCalls { callbacks: Some(callbacks), clippy_preference }, file_loader.into_boxed())
        }
        None => (ShimCalls::default(), None),
    };
    #[cfg(not(feature = "ipc"))]
    let (mut shim_calls, file_loader) = (ShimCalls::default(), None);

    let args = env::args_os()
        .enumerate()
        .map(|(i, arg)| {
            arg.into_string().unwrap_or_else(|arg| {
                early_error(
                    ErrorOutputType::default(),
                    &format!("Argument {} is not valid Unicode: {:?}", i, arg),
                )
            })
        })
        .collect::<Vec<_>>();

    #[cfg(feature = "clippy")]
    let args = match clippy_preference {
        Some(preference) => clippy::adjust_args(args, preference),
        None => args,
    };
    dbg!(&args);

    let result = rustc_driver::report_ices_to_stderr_if_any(|| {
        run_compiler(&args, &mut shim_calls, file_loader, None)
    })
    .and_then(|result| result);

    #[cfg(feature = "ipc")]
    std::mem::drop(rt);

    process::exit(result.is_err() as i32);
}

#[derive(Default)]
struct ShimCalls {
    #[cfg(feature = "ipc")]
    callbacks: Option<ipc::IpcCallbacks>,
    clippy_preference: Option<clippy::ClippyPreference>,
}

impl Callbacks for ShimCalls {
    fn config(&mut self, config: &mut interface::Config) {
        config.opts.debugging_opts.continue_parse_after_error = true;
        config.opts.debugging_opts.save_analysis = true;
    }

    #[cfg(feature = "clippy")]
    fn after_parsing(&mut self, compiler: &interface::Compiler) -> Compilation {
        match self.clippy_preference {
            Some(preference) if preference != clippy::ClippyPreference::Off => {
                clippy::after_parse_callback(compiler);
            }
            _ => {}
        }

        Compilation::Continue
    }

    fn after_analysis(&mut self, compiler: &interface::Compiler) -> Compilation {
        let callbacks = match self.callbacks.as_ref() {
            Some(callbacks) => callbacks,
            None => return Compilation::Continue,
        };

        use rustc_save_analysis::CallbackHandler;

        let sess = compiler.session();
        let input = compiler.input();
        let crate_name = compiler.crate_name().unwrap().peek().clone();

        let cwd = &sess.working_dir.0;

        let src_path = match input {
            Input::File(ref name) => Some(name.to_path_buf()),
            Input::Str { .. } => None,
        }
        .and_then(|path| src_path(Some(cwd), path));

        let krate = Crate {
            name: crate_name.to_owned(),
            src_path,
            disambiguator: sess.local_crate_disambiguator().to_fingerprint().as_value(),
            edition: match sess.edition() {
                syntax::edition::Edition::Edition2015 => Edition::Edition2015,
                syntax::edition::Edition::Edition2018 => Edition::Edition2018,
            },
        };

        let mut input_files: HashMap<PathBuf, HashSet<Crate>> = HashMap::new();
        for file in fetch_input_files(sess) {
            input_files.entry(file).or_default().insert(krate.clone());
        }

        eprintln!(">>> Client: Call input_files");
        if let Err(e) = callbacks.input_files(input_files).wait() {
            eprintln!("Can't send input files as part of a compilation callback: {:?}", e);
        }

        // Guaranteed to not be dropped yet in the pipeline thanks to the
        // `config.opts.debugging_opts.save_analysis` value being set to `true`.
        let expanded_crate = &compiler.expansion().unwrap().peek().0;
        compiler.global_ctxt().unwrap().peek_mut().enter(|tcx| {
            // There are two ways to move the data from rustc to the RLS, either
            // directly or by serialising and deserialising. We only want to do
            // the latter when there are compatibility issues between crates.

            // This version passes via JSON, it is more easily backwards compatible.
            // save::process_crate(state.tcx.unwrap(),
            //                     state.expanded_crate.unwrap(),
            //                     state.analysis.unwrap(),
            //                     state.crate_name.unwrap(),
            //                     state.input,
            //                     None,
            //                     save::DumpHandler::new(state.out_dir,
            //                                            state.crate_name.unwrap()));
            // This version passes directly, it is more efficient.
            rustc_save_analysis::process_crate(
                tcx,
                &expanded_crate,
                &crate_name,
                &input,
                None,
                CallbackHandler {
                    callback: &mut |a| {
                        eprintln!(">>> Client: Entered CallbackHandler::callback");
                        let analysis = unsafe { ::std::mem::transmute(a.clone()) };
                        if let Err(e) = callbacks.complete_analysis(analysis).wait() {
                            eprintln!(
                                "Can't send analysis as part of a compilation callback: {:?}",
                                e
                            );
                        }
                    },
                },
            );
        });

        Compilation::Continue
    }
}

fn fetch_input_files(sess: &Session) -> Vec<PathBuf> {
    let cwd = &sess.working_dir.0;

    sess.source_map()
        .files()
        .iter()
        .filter(|fmap| fmap.is_real_file())
        .filter(|fmap| !fmap.is_imported())
        .map(|fmap| fmap.name.to_string())
        .map(|fmap| src_path(Some(cwd), fmap).unwrap())
        .collect()
}

pub fn src_path(cwd: Option<&Path>, path: impl AsRef<Path>) -> Option<PathBuf> {
    let path = path.as_ref();

    Some(match (cwd, path.is_absolute()) {
        (_, true) => path.to_owned(),
        (Some(cwd), _) => cwd.join(path),
        (None, _) => std::env::current_dir().ok()?.join(path),
    })
}
