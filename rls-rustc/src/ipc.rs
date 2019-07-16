use jsonrpc_core::types::params::Params;
use jsonrpc_core_client::transports::duplex;
use jsonrpc_core_client::RpcChannel;
use jsonrpc_core_client::RpcError;
use jsonrpc_core_client::TypedClient;
use std::io;
use std::path::{Path, PathBuf};

use crate::syntax::source_map::FileLoader;
use futures::Future;
use parity_tokio_ipc::IpcConnection;

use tokio::io::AsyncRead;

// use jsonrpc_client_core::jsonrpc_client;
use jsonrpc_core::{Error, IoHandler, Result};
use jsonrpc_core_client::transports::local;
use jsonrpc_derive::rpc;

#[rpc]
pub trait Rpc {
    /// Returns a protocol version
    #[rpc(name = "say_hello")]
    fn say_hello(&self) -> Result<String>;
}

pub struct IpcFileLoader;

impl IpcFileLoader {
    pub fn new(path: String) -> io::Result<Self> {
        let mut runtime = tokio::runtime::Runtime::new().unwrap();

        eprintln!("ipc: Attempting to connect to {}", path);
        let connection = IpcConnection::connect(path, &Default::default())?;
        use futures::stream::Stream;
        use futures::sink::Sink;
        // let codec = jsonrpc_server_utilscodecs::StreamCodec::stream_incoming();
        let codec = tokio::codec::LinesCodec::new();
        let (sink, stream) = connection
            .framed(codec)
            .split();
        let sink = sink.sink_map_err(|_| jsonrpc_core_client::RpcError::Timeout);
        let stream = stream.map_err(|_| jsonrpc_core_client::RpcError::Timeout);
        eprintln!("ipc: Client connected");

        eprintln!("ipc: Setting up duplex");
        let (client, sender) = duplex(sink, stream);

        let raw = jsonrpc_core_client::RawClient::from(sender);
        eprintln!("ipc: Call say_hello");
        let result = raw
            .call_method("say_hello", Params::None)
            .map(|val| {
                eprintln!("ipc: Result of say_hello method: {:?}", val);
            })
            .map_err(|e| {
                eprintln!("ipc: Called method say_hello failed with: {:?}", e)
            });

        // let core = tokio::reactor::Reactor::new().unwrap();
        runtime.spawn(client.map_err(|_| ()));
        let blocked_on = runtime.block_on(result);
        dbg!(&blocked_on);

        // let result = runtime.block_on(result).unwrap();
        // dbg!(&result);

        // struct TestClient(TypedClient);
        // impl TestClient {
        //     fn say_hello(&self) -> impl Future<Item = String, Error = RpcError> {
        //         self.0.call_method("say_hello", "()", ())
        //     }
        //     // fn hello(&self, msg: &'static str) -> impl Future<Item = String, Error = RpcError> {
        //     //     self.0.call_method("hello", "String", (msg,))
        //     // }
        //     // fn fail(&self) -> impl Future<Item = (), Error = RpcError> {
        //     //     self.0.call_method("fail", "()", ())
        //     // }
        // }
        // let test_client = TestClient(sender.into());
        // let call_result = test_client.say_hello().wait().unwrap();
        // dbg!(&call_result);
        // let client = TClient::from(sender);
        // (client, rpc_client)

        // duplex::duplex(, stream: TStream)
        // let (client, server) = duplex::connect::<gen_client::Client, _, _>(io);

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

        // let rx_buf2 = vec![0u8; 5];
        // tokio::io::write_all(connection, b"Dupaa").wait();
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
