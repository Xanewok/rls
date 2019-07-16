use std::io;
use std::path::{Path, PathBuf};

use crate::syntax::source_map::FileLoader;
use futures::Future;
use parity_tokio_ipc::IpcConnection;

pub struct IpcFileLoader;

impl IpcFileLoader {
    pub fn new(path: String) -> io::Result<Self> {
        eprintln!("ipc: Attempting to connect to {}", path);
        let connection = IpcConnection::connect(path, &Default::default())?;
        eprintln!("ipc: Client connected");

        // let msg = "client here";
        // let rx_buf = vec![0u8; msg.len()];
        // let client_0_fut = tokio::io::write_all(connection, msg)
        //     .map_err(|err| panic!("Client 0 write error: {:?}", err))
        //     .and_then(move |(client, _)| {
        //         tokio::io::read_exact(client, rx_buf).map(|(_, buf)| buf)
        //             .map_err(|err| panic!("Client 0 read error: {:?}", err))
        //     });
        // eprintln!("ipc: Waiting on client_0");
        // client_0_fut.wait().unwrap();

        let rx_buf2 = vec![0u8; 5];
        tokio::io::write_all(connection, b"Dupaa").wait();
        // tokio::io::read(connection, rx_buf2).map(|(_, buf, _)| dbg!(buf));

        // let fut = tokio::io::read(connection, rx_buf2)
        //     .map(|(_conn, buf, _bytes)| buf)
        //     .map_err(|err| panic!("Client 1 read error: {:?}", err));
        // eprintln!("Waiting on test_buf");
        // let test_buf = fut.wait().unwrap();

        // eprintln!("Read from IPC: `{}`", String::from_utf8(test_buf).unwrap());

        Ok(IpcFileLoader)
    }
}

impl FileLoader for IpcFileLoader {
    fn file_exists(&self, _path: &Path) -> bool {
        unimplemented!()
    }

    fn abs_path(&self, _path: &Path) -> Option<PathBuf> {
        unimplemented!()
    }

    fn read_file(&self, _path: &Path) -> io::Result<String> {
        unimplemented!()
    }
}
