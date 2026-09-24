use base64::{engine::general_purpose::STANDARD, Engine as _};
use kosmos_codewars_worker::{DataError, MAX_RESPONSE_BYTES, ORIGIN, SYNC_KEY};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufWriter, Write},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

mod sync;

#[derive(Debug, Deserialize)]
struct Bootstrap {
    package_id: String,
    version: String,
    hash: String,
    pid: u32,
    api_version: u32,
    generation: u64,
    token: String,
    integration: Option<IntegrationBootstrap>,
}
#[derive(Debug, Deserialize)]
struct IntegrationBootstrap {
    values: BTreeMap<String, String>,
}
#[derive(Debug)]
enum WorkerError {
    Io,
    Json,
    Stopped,
    Host,
    Invalid,
}
impl From<DataError> for WorkerError {
    fn from(error: DataError) -> Self {
        match error {
            DataError::Invalid => Self::Invalid,
            DataError::Json => Self::Json,
        }
    }
}

struct Client<'a> {
    input: io::Lines<io::StdinLock<'a>>,
    output: Arc<Mutex<BufWriter<io::Stdout>>>,
    token: String,
    generation: u64,
    next_id: u64,
}
impl<'a> Client<'a> {
    fn send(output: &Arc<Mutex<BufWriter<io::Stdout>>>, message: Value) -> Result<(), WorkerError> {
        let mut output = output.lock().map_err(|_| WorkerError::Io)?;
        serde_json::to_writer(&mut *output, &message).map_err(|_| WorkerError::Json)?;
        output.write_all(b"\n").map_err(|_| WorkerError::Io)?;
        output.flush().map_err(|_| WorkerError::Io)
    }
    fn next_message(&mut self) -> Result<Option<Value>, WorkerError> {
        self.input
            .next()
            .transpose()
            .map_err(|_| WorkerError::Io)?
            .map(|line| serde_json::from_str(&line).map_err(|_| WorkerError::Json))
            .transpose()
    }
    fn call(&mut self, operation: &str, params: Value) -> Result<Value, WorkerError> {
        let id = format!("codewars-{}", self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        Self::send(
            &self.output,
            json!({"method":"worker.call","id":id,"generation":self.generation,"token":self.token,"operation":operation,"params":params}),
        )?;
        for line in self.input.by_ref() {
            let message: Value = serde_json::from_str(&line.map_err(|_| WorkerError::Io)?)
                .map_err(|_| WorkerError::Json)?;
            match message.get("method").and_then(Value::as_str) {
                Some("worker.stop") => return Err(WorkerError::Stopped),
                Some("worker.run") => continue,
                Some("worker.result")
                    if message.get("id").and_then(Value::as_str) == Some(id.as_str()) =>
                {
                    return message
                        .get("ok")
                        .and_then(Value::as_bool)
                        .filter(|ok| *ok)
                        .and_then(|_| message.get("result").cloned())
                        .ok_or(WorkerError::Host)
                }
                _ => {}
            }
        }
        Err(WorkerError::Io)
    }
    fn fetch(&mut self, url: String) -> Result<Vec<u8>, WorkerError> {
        if !url.starts_with(&format!("{ORIGIN}/")) {
            return Err(WorkerError::Invalid);
        }
        let result = self.call("network.fetch", json!({"url":url}))?;
        let bytes = result
            .get("bytes")
            .and_then(Value::as_str)
            .and_then(|value| STANDARD.decode(value).ok())
            .ok_or(WorkerError::Host)?;
        (bytes.len() <= MAX_RESPONSE_BYTES)
            .then_some(bytes)
            .ok_or(WorkerError::Invalid)
    }
    fn ark_read(&mut self, operation: &str, params: Value) -> Result<Value, WorkerError> {
        self.call("ark.read", json!({"operation":operation,"params":params}))
    }
    fn ark_write(&mut self, operation: &str, params: Value) -> Result<Value, WorkerError> {
        self.call("ark.write", json!({"operation":operation,"params":params}))
    }
    fn sync_value(&mut self) -> Result<Option<String>, WorkerError> {
        self.ark_read("get_sync_kv", json!({"key":SYNC_KEY}))
            .map(|value| value.as_str().map(str::to_owned))
    }
}

fn heartbeat(output: Arc<Mutex<BufWriter<io::Stdout>>>, generation: u64, token: String) {
    loop {
        thread::sleep(Duration::from_secs(15));
        if Client::send(
            &output,
            json!({"method":"worker.heartbeat","generation":generation,"token":token}),
        )
        .is_err()
        {
            return;
        }
    }
}
fn run() -> Result<(), WorkerError> {
    let stdin = io::stdin();
    let mut input = stdin.lock().lines();
    let bootstrap: Bootstrap = serde_json::from_str(
        &input
            .next()
            .ok_or(WorkerError::Io)?
            .map_err(|_| WorkerError::Io)?,
    )
    .map_err(|_| WorkerError::Json)?;
    let username = bootstrap
        .integration
        .as_ref()
        .and_then(|integration| integration.values.get("username"))
        .filter(|value| kosmos_codewars_worker::valid_username(value))
        .cloned()
        .ok_or(WorkerError::Invalid)?;
    let output = Arc::new(Mutex::new(BufWriter::new(io::stdout())));
    Client::send(
        &output,
        json!({"method":"worker.hello","package_id":bootstrap.package_id,"version":bootstrap.version,"hash":bootstrap.hash,"pid":bootstrap.pid,"api_version":bootstrap.api_version,"token":bootstrap.token}),
    )?;
    let heartbeat_output = Arc::clone(&output);
    let heartbeat_token = bootstrap.token.clone();
    thread::spawn(move || heartbeat(heartbeat_output, bootstrap.generation, heartbeat_token));
    let generation = bootstrap.generation;
    let token = bootstrap.token;
    let mut client = Client {
        input,
        output,
        token,
        generation,
        next_id: 0,
    };
    while let Some(message) = client.next_message()? {
        match message.get("method").and_then(Value::as_str) {
            Some("worker.stop") => return Ok(()),
            Some("worker.run")
                if message.get("generation").and_then(Value::as_u64) == Some(generation) =>
            {
                if let Err(error) = sync::run(&mut client, &username) {
                    if matches!(error, WorkerError::Stopped) {
                        return Ok(());
                    }
                    eprintln!("Codewars sync failed");
                }
            }
            _ => {}
        }
    }
    Ok(())
}
fn main() {
    let _ = run();
}
