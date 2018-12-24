use lsp_client;
use tokio::runtime::current_thread::Runtime;

#[test]
fn client() {
	use std::process::Command;
	let mut runtime = Runtime::new().unwrap();

	let mut cmd = Command::new("echo");
	cmd.arg("ohai there");

	let mut client = lsp_client::SimpleClient::spawn_from_command(cmd);

	client.receive_with_runtime(&mut runtime, None).unwrap();
}
