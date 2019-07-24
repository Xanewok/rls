#![feature(rustc_private)]

extern crate env_logger;
extern crate rustc;
extern crate rustc_driver;
extern crate rustc_interface;
extern crate rustc_save_analysis;
extern crate syntax;

use rustc::session::config::ErrorOutputType;
use rustc::session::early_error;
use rustc_driver::{run_compiler, Callbacks, Compilation};
use rustc_interface::interface;

use std::{env, process};

#[cfg(feature = "ipc")]
mod ipc;

pub fn run() {
    let _ = env_logger::try_init(); // TODO: Remove me

    #[cfg(feature = "ipc")]
    let (mut shim_calls, file_loader, runtime) = {
        let mut rt = tokio::runtime::Runtime::new().unwrap();
        // (
            let (client, file_loader) = match env::var("RLS_IPC_ENDPOINT").ok() {
                Some(endpoint) => {
                    #[allow(deprecated)] // Windows doesn't work with lazily-bound reactors
                    let reactor = rt.reactor().clone();
                    let client: ipc::FileLoader = rt.block_on(ipc::connect(endpoint.into(), reactor)).expect("Couldn't connecto IPC endpoint");

                    (ShimCalls(Some(client.clone())), client.into_boxed())
                },
                None => (ShimCalls(None), None),
            };

            (client, file_loader, rt)

            // env::var("RLS_IPC_ENDPOINT")
            //     .ok()
            //     .map(|endpoint| {
            //         #[allow(deprecated)] // Windows doesn't work with lazily-bound reactors
            //         let reactor = rt.reactor().clone();
            //         let client: ipc::FileLoader = rt.block_on(ipc::connect(endpoint.into(), reactor)).expect("Couldn't connecto IPC endpoint");
            //         (client.clone(), client.into_boxed())
            //         // ipc::FileLoader::spawn(endpoint.into(), &mut rt)
            //         //     .map_err(|e| log::warn!("Couldn't connect to IPC endpoint: {:?}", e))
            //             // .ok()
            //     })
            //     // .map(ipc::FileLoader::into_boxed)
            //     .unwrap_or(None),
            // rt,
        // )
    };
    #[cfg(not(feature = "ipc"))]
    let (mut shim_calls, file_loader) = (ShimCalls, None);

    let result = rustc_driver::report_ices_to_stderr_if_any(|| {
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

        run_compiler(&args, &mut shim_calls, file_loader, None)
    })
    .and_then(|result| result);

    #[cfg(feature = "ipc")]
    futures::future::Future::wait(runtime.shutdown_now()).unwrap();

    process::exit(result.is_err() as i32);
}

#[cfg(feature = "ipc")]
struct ShimCalls(Option<ipc::FileLoader>);
#[cfg(not(feature = "ipc"))]
struct ShimCalls;

impl Callbacks for ShimCalls {
    fn config(&mut self, config: &mut interface::Config) {
        config.opts.debugging_opts.continue_parse_after_error = true;
        config.opts.debugging_opts.save_analysis = true;
    }

    fn after_analysis(&mut self, compiler: &interface::Compiler) -> Compilation {
        let client = match self.0.as_ref() {
            Some(client) => client,
            None => return Compilation::Continue,
        };

        use rustc_save_analysis::CallbackHandler;

        // let sess = compiler.session();
        let input = compiler.input();
        let crate_name = compiler.crate_name().unwrap().peek().clone();

        // let cwd = &sess.working_dir.0;

        // let src_path = match input {
        //     Input::File(ref name) => Some(name.to_path_buf()),
        //     Input::Str { .. } => None,
        // }
        // .and_then(|path| src_path(Some(cwd), path));

        // let krate = Crate {
        //     name: crate_name.to_owned(),
        //     src_path,
        //     disambiguator: sess.local_crate_disambiguator().to_fingerprint().as_value(),
        //     edition: match sess.edition() {
        //         RustcEdition::Edition2015 => Edition::Edition2015,
        //         RustcEdition::Edition2018 => Edition::Edition2018,
        //     },
        // };

        // let mut input_files = self.input_files.lock().unwrap();
        // for file in fetch_input_files(sess) {
        //     input_files.entry(file).or_default().insert(krate.clone());
        // }

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
                        use futures::future::Future;
                        eprintln!(">>> Client: Entered CallbackHandler::callback");
                        if let Err(e) = client.complete_analysis(unsafe { ::std::mem::transmute(a.clone()) }).wait() {
                            eprintln!("Can't send analysis as part of a compilation callback: {:?}", e);
                        }
                        // tokio::spawn(request.map_err(|_| ())).wait();
                        // let mut analysis = self.analysis.lock().unwrap();
                        // let a = unsafe { ::std::mem::transmute(a.clone()) };
                        // *analysis = Some(a);
                    },
                },
            );
        });

        Compilation::Continue
    }
}