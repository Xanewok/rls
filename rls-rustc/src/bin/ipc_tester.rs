// TODO: Remove me, this is only here for demonstration purposes how to set up
// a server.

use std::process::Command;

use futures::{stream::Stream, sync::oneshot, Future};
use parity_tokio_ipc::{dummy_endpoint, Endpoint, SecurityAttributes};
use tokio;
use tokio::io::{self, AsyncRead};

use jsonrpc_ipc_server::jsonrpc_core::*;
use jsonrpc_ipc_server::ServerBuilder;
use jsonrpc_derive::rpc;

#[rpc]
pub trait Rpc {
    /// Returns a protocol version
    #[rpc(name = "say_hello")]
    fn say_hello(&self) -> Result<String>;
}
struct RpcImpl;
impl Rpc for RpcImpl {
    fn say_hello(&self) -> Result<String> { Ok(String::from("No siema")) }
}

fn main() {
    env_logger::init();

    let endpoint_path = dummy_endpoint();
    
    let thread = std::thread::spawn({
        let endpoint_path = endpoint_path.clone();
        move || {
            let mut io = IoHandler::new();
            io.add_method("say_hello", |_params| {
                eprintln!("ipc_tester: At long fucking last");
                Ok(serde_json::Value::String("No eloszka".into()))
            });
            // io.extend_with(RpcImpl.to_delegate());

            let builder = ServerBuilder::new(io);
            let server = builder.start(&endpoint_path).expect("Couldn't open socket");
            eprintln!("ipc_tester: About to wait on the IPC server");
            server.wait();

            // let connections = endpoint
            //     .incoming(&Default::default())
            //     .expect("failed to open up a new pipe/socket");
            // let server = connections
            //     .for_each(|(stream, _)| {
            //         eprintln!("ipc_tester: Client has connected!");
            //         let (reader, writer) = stream.split();

            //             let buf = vec![0; 128];
            //             io::read(reader, buf)
            //                 .map(|(_, buf, _)| eprintln!("ipc_tester: Server read {:?}", buf)).wait();
            //                 Ok(())
            //         // io::write_all(writer, b"Server sending hello!").and_then(|_| {
            //         // })
            //     })
            //     .map_err(|_| ());
            // executor.spawn(server);
            // tx.send(()).expect("failed to send ok");
        }
    });
    // rx.wait().expect("Failed to wait for the server");
    eprintln!("ipc_tester: Server has been spawned");

    std::thread::sleep_ms(1000);

    let output = Command::new("cargo")
        .args(&["run", "--bin", "rustc"])
        .env("RLS_IPC_ENDPOINT", endpoint_path)
        .stderr(std::process::Stdio::inherit())
        .output()
        .unwrap();
    dbg!(&output);

    // runtime.shutdown_on_idle().wait().unwrap();
    let _ = thread.join();
}
