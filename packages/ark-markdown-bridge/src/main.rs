// Keep the shipped bridge binary aligned with the exercised first-party worker.
#[path = "../../../runtime/src/bin/ark-markdown-bridge.rs"]
mod worker;

fn main() {
    worker::main();
}
