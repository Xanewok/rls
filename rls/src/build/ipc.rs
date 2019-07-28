use std::collections::{HashMap, HashSet};
use std::ops::DerefMut;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::{env, fs};

use jsonrpc_core::{ErrorCode, IoHandler};
use rls_vfs::{FileContents, Vfs};

use crate::build::plan::Crate;

use rls_ipc::rpc::{self, Error, Result as RpcResult};
use rls_ipc::server::{ServerBuilder, CloseHandle};

/// TODO: Document me
pub fn start_with_all(
    vfs: Arc<Vfs>,
    analysis: Arc<Mutex<Option<rls_data::Analysis>>>,
    input_files: Arc<Mutex<HashMap<PathBuf, HashSet<Crate>>>>,
) -> Result<(PathBuf, CloseHandle), ()> {
    use rls_ipc::rpc::callbacks::Server as _;
    use rls_ipc::rpc::file_loader::Server as _;

    let mut io = IoHandler::new();
    io.extend_with(ArcVfs(vfs).to_delegate());
    io.extend_with(callbacks::CallbackHandler { analysis, input_files }.to_delegate());

    self::start_with_handler(io)
}

/// Spins up an IPC server in the background. Currently used for inter-process
/// VFS, which is required for out-of-process rustc compilation.
pub fn start(vfs: Arc<Vfs>) -> Result<(PathBuf, CloseHandle), ()> {
    let mut io = IoHandler::new();
    use rls_ipc::rpc::file_loader::Server as _;
    io.extend_with(ArcVfs(vfs).to_delegate());

    self::start_with_handler(io)
}

/// Spins up an IPC server in the background.
pub fn start_with_handler(io: IoHandler) -> Result<(PathBuf, CloseHandle), ()> {
    let endpoint_path = gen_endpoint_path();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn({
        let endpoint_path = endpoint_path.clone();
        move || {
            log::trace!("Attempting to spin up IPC server at {}", endpoint_path);
            let runtime = tokio::runtime::Builder::new()
                .core_threads(1)
                .build()
                .unwrap();
            #[allow(deprecated)] // Windows won't work with lazily bound reactor
            let (reactor, executor) = (runtime.reactor(), runtime.executor());

            let server = ServerBuilder::new(io)
                .event_loop_executor(executor)
                .event_loop_reactor(reactor.clone())
                .start(&endpoint_path)
                .map_err(|_| log::warn!("Couldn't open socket"))
                .unwrap();
            log::trace!("Started the IPC server at {}", endpoint_path);

            tx.send(server.close_handle()).unwrap();
            server.wait();
        }
    });

    rx.recv_timeout(Duration::from_secs(5))
        .map(|handle| (endpoint_path.into(), handle))
        .map_err(|_| ())
}

fn gen_endpoint_path() -> String {
    let num: u64 = rand::Rng::gen(&mut rand::thread_rng());
    if cfg!(windows) {
        format!(r"\\.\pipe\ipc-pipe-{}", num)
    } else {
        format!(r"/tmp/ipc-uds-{}", num)
    }
}

fn rpc_error(msg: &str) -> Error {
    Error { code: ErrorCode::InternalError, message: msg.to_owned(), data: None }
}

struct ArcVfs(Arc<Vfs>);

impl rpc::file_loader::Rpc for ArcVfs {
    fn file_exists(&self, path: PathBuf) -> RpcResult<bool> {
        log::debug!(">>>> Server: file_exists({:?})", path);
        // Copied from syntax::source_map::RealFileLoader
        Ok(fs::metadata(path).is_ok())
    }
    fn abs_path(&self, path: PathBuf) -> RpcResult<Option<PathBuf>> {
        log::debug!(">>>> Server: abs_path({:?})", path);
        // Copied from syntax::source_map::RealFileLoader
        Ok(if path.is_absolute() {
            Some(path.to_path_buf())
        } else {
            env::current_dir().ok().map(|cwd| cwd.join(path))
        })
    }
    fn read_file(&self, path: PathBuf) -> RpcResult<String> {
        log::debug!(">>>> Server: read_file({:?})", path);
        self.0.load_file(&path).map_err(|e| rpc_error(&e.to_string())).and_then(|contents| {
            match contents {
                FileContents::Text(text) => Ok(text),
                FileContents::Binary(..) => Err(rpc_error("File is binary")),
            }
        })
    }
}

mod callbacks {
    use super::{Arc, Mutex};
    use super::{HashMap, HashSet};
    use super::{PathBuf};
    use super::{rpc, RpcResult};
    // use crate::build::plan::Crate;
    impl From<rls_ipc::rpc::Crate> for crate::build::plan::Crate {
        fn from(krate: rls_ipc::rpc::Crate) -> Self {
            Self {
                name: krate.name,
                src_path: krate.src_path,
                edition: match krate.edition {
                    rls_ipc::rpc::Edition::Edition2015 => crate::build::plan::Edition::Edition2015,
                    rls_ipc::rpc::Edition::Edition2018 => crate::build::plan::Edition::Edition2018,
                },
                disambiguator: krate.disambiguator,
            }
        }
    }

    pub struct CallbackHandler {
        pub analysis: Arc<Mutex<Option<rls_data::Analysis>>>,
        pub input_files: Arc<Mutex<HashMap<PathBuf, HashSet<crate::build::plan::Crate>>>>,
    }

    impl rpc::callbacks::Rpc for CallbackHandler {
        fn complete_analysis(&self, analysis: rls_data::Analysis) -> RpcResult<()> {
            log::debug!(">>>> Server: complete_analysis({:?})", analysis.compilation.as_ref().map(|comp| comp.output.clone()));
            *self.analysis.lock().unwrap() = Some(analysis);
            Ok(())
        }

        fn input_files(&self, input_files: HashMap<PathBuf, HashSet<rls_ipc::rpc::Crate>>) -> RpcResult<()> {
            log::debug!(">>>> Server: input_files({:?})", &input_files);
            let mut current_files = self.input_files.lock().unwrap();
            for (file, crates) in input_files {
                current_files.entry(file).or_default().extend(crates.into_iter().map(From::from));
            }
            Ok(())
        }
    }
}
