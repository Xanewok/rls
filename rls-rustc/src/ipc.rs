use std::io;
use std::path::{Path, PathBuf};
use std::collections::{HashMap, HashSet};

use failure::Fail;
use futures::Future;

pub use rls_ipc::client::{connect, Client, RpcChannel, RpcError};

#[derive(Clone)]
pub struct FileLoader(Client);

impl From<RpcChannel> for FileLoader {
    fn from(channel: RpcChannel) -> Self {
        FileLoader(channel.into())
    }
}

impl FileLoader {
    pub fn spawn(path: PathBuf, runtime: &mut tokio::runtime::Runtime) -> io::Result<Self> {
        #[allow(deprecated)] // Windows doesn't work with lazily-bound reactors
        let reactor = runtime.reactor().clone();
        let connection = self::connect(path, &reactor)?;

        Ok(
            runtime.block_on(connection)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e.compat()))?
        )
    }

    pub fn into_boxed(self) -> Option<Box<dyn syntax::source_map::FileLoader + Send + Sync>> {
        Some(Box::new(self))
    }
}

impl FileLoader {
    pub fn file_exists(&self, path: PathBuf) -> impl Future<Item = bool, Error = RpcError> {
        eprintln!(">>>> Client: file_exists({:?})", &path);
        self.0.file_loader.file_exists(path)
        // self.0.call_method("file_exists", "bool", (path,))
    }

    pub fn abs_path(&self, path: PathBuf) -> impl Future<Item = Option<PathBuf>, Error = RpcError> {
        eprintln!(">>>> Client: abs_path({:?})", &path);
        self.0.file_loader.abs_path(path)
    }

    pub fn read_file(&self, path: PathBuf) -> impl Future<Item = String, Error = RpcError> {
        eprintln!(">>>> Client: read_file({:?})", &path);
        self.0.file_loader.read_file(path)
    }

    pub fn complete_analysis(&self,  analysis: rls_data::Analysis) -> impl Future<Item = (), Error = RpcError> {
        eprintln!(">>>> Client: complete_analysis({:?})", analysis.compilation.as_ref().map(|comp| comp.output.clone()));
        self.0.callbacks.complete_analysis(analysis)
    }

    pub fn input_files(&self, input_files: HashMap<PathBuf, HashSet<rls_ipc::rpc::Crate>>) -> impl Future<Item = (), Error = RpcError> {
        eprintln!(">>>> Client: input_files({:?})", &input_files);
        self.0.callbacks.input_files(input_files)
    }
}

impl syntax::source_map::FileLoader for FileLoader {
    fn file_exists(&self, path: &Path) -> bool {
        self.file_exists(path.to_owned()).wait().unwrap()
    }

    fn abs_path(&self, path: &Path) -> Option<PathBuf> {
        self.abs_path(path.to_owned()).wait().ok()?
    }

    fn read_file(&self, path: &Path) -> io::Result<String> {
        self.read_file(path.to_owned())
            .wait()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e.compat()))
    }
}
