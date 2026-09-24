use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::Utc;
use kosmos_bigfrontend_worker::{
    cutoff, encode_path_segment, map_submission, profile_json, valid_username, value_id,
    CODING_SUBMISSION_TYPE_ID, MAX_ITEMS, MAX_RESPONSE_BYTES, ORIGIN, PROFILE_PATH, SYNC_KEY,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufWriter, Write},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

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

impl From<kosmos_bigfrontend_worker::DataError> for WorkerError {
    fn from(error: kosmos_bigfrontend_worker::DataError) -> Self {
        match error {
            kosmos_bigfrontend_worker::DataError::Json => Self::Json,
            kosmos_bigfrontend_worker::DataError::Invalid => Self::Invalid,
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
    fn next_message(&mut self) -> Result<Option<Value>, WorkerError> {
        self.input
            .next()
            .transpose()
            .map_err(|_| WorkerError::Io)?
            .map(|line| serde_json::from_str(&line).map_err(|_| WorkerError::Json))
            .transpose()
    }

    fn send(output: &Arc<Mutex<BufWriter<io::Stdout>>>, message: Value) -> Result<(), WorkerError> {
        let mut output = output.lock().map_err(|_| WorkerError::Io)?;
        serde_json::to_writer(&mut *output, &message).map_err(|_| WorkerError::Json)?;
        output.write_all(b"\n").map_err(|_| WorkerError::Io)?;
        output.flush().map_err(|_| WorkerError::Io)
    }

    fn call(&mut self, operation: &str, params: Value) -> Result<Value, WorkerError> {
        let id = format!("bigfrontend-{}", self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        Self::send(
            &self.output,
            json!({
                "method": "worker.call",
                "id": id,
                "generation": self.generation,
                "token": self.token,
                "operation": operation,
                "params": params
            }),
        )?;
        for line in self.input.by_ref() {
            let line = line.map_err(|_| WorkerError::Io)?;
            let message: Value = serde_json::from_str(&line).map_err(|_| WorkerError::Json)?;
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
                        .ok_or(WorkerError::Host);
                }
                _ => {}
            }
        }
        Err(WorkerError::Io)
    }

    fn ark_write(&mut self, operation: &str, params: Value) -> Result<Value, WorkerError> {
        self.call(
            "ark.write",
            json!({"operation": operation, "params": params}),
        )
    }

    fn sync_value(&mut self) -> Result<Option<String>, WorkerError> {
        self.call(
            "ark.read",
            json!({"operation":"get_sync_kv","params":{"key":SYNC_KEY}}),
        )
        .map(|value| value.as_str().map(str::to_owned))
    }

    fn fetch(&mut self, url: &str) -> Result<Vec<u8>, WorkerError> {
        if !url
            .strip_prefix(ORIGIN)
            .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return Err(WorkerError::Invalid);
        }
        let result = self.call("network.fetch", json!({"url": url}))?;
        let bytes = result
            .get("bytes")
            .and_then(Value::as_str)
            .and_then(|value| STANDARD.decode(value).ok())
            .ok_or(WorkerError::Host)?;
        (bytes.len() <= MAX_RESPONSE_BYTES)
            .then_some(bytes)
            .ok_or(WorkerError::Invalid)
    }
}

fn sync(client: &mut Client<'_>, username: &str) -> Result<(), WorkerError> {
    let cutoff = cutoff(client.sync_value()?.as_deref());
    let profile_url = format!("{ORIGIN}{PROFILE_PATH}{}", encode_path_segment(username));
    let profile = profile_json(
        std::str::from_utf8(&client.fetch(&profile_url)?).map_err(|_| WorkerError::Invalid)?,
    )?;
    let user_id =
        value_id(&profile["props"]["pageProps"]["profile"]["id"]).ok_or(WorkerError::Invalid)?;
    let activity_url = format!(
        "{ORIGIN}/api/activity?type=submission&userId={}",
        encode_path_segment(&user_id)
    );
    let activity: Value =
        serde_json::from_slice(&client.fetch(&activity_url)?).map_err(|_| WorkerError::Json)?;
    let items = activity
        .get("items")
        .and_then(Value::as_array)
        .ok_or(WorkerError::Invalid)?;
    if items.len() > MAX_ITEMS {
        return Err(WorkerError::Invalid);
    }
    let now = Utc::now().to_rfc3339();
    client.ark_write(
        "upsert_object_type",
        json!({
            "object_type": {
                "id": CODING_SUBMISSION_TYPE_ID,
                "name": "Отправка задачи",
                "schemaJson": "{}",
                "uiSchemaJson": "{}",
                "createdAt": now,
                "updatedAt": now,
                "systemLocked": false
            }
        }),
    )?;
    for item in items {
        if cutoff.is_some_and(|at| {
            kosmos_bigfrontend_worker::timestamp_for_cutoff(item)
                .is_some_and(|item_at| item_at < at)
        }) {
            continue;
        }
        client.ark_write("upsert_object", json!({"object": map_submission(item)?}))?;
    }
    client.ark_write("set_sync_kv", json!({"key":SYNC_KEY,"value":now}))?;
    Ok(())
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
        .filter(|username| valid_username(username))
        .ok_or(WorkerError::Invalid)?
        .clone();
    let output = Arc::new(Mutex::new(BufWriter::new(io::stdout())));
    let package_id = bootstrap.package_id.clone();
    let version = bootstrap.version.clone();
    let hash = bootstrap.hash.clone();
    let token = bootstrap.token.clone();
    Client::send(
        &output,
        json!({
            "method": "worker.hello",
            "package_id": package_id,
            "version": version,
            "hash": hash,
            "pid": bootstrap.pid,
            "api_version": bootstrap.api_version,
            "token": token
        }),
    )?;
    let heartbeat_output = Arc::clone(&output);
    thread::spawn(move || heartbeat(heartbeat_output, bootstrap.generation, bootstrap.token));
    let generation = bootstrap.generation;
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
                if let Err(error) = sync(&mut client, &username) {
                    if matches!(error, WorkerError::Stopped) {
                        return Ok(());
                    }
                    eprintln!("BigFrontend sync failed");
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
