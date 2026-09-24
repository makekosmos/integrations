use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::Utc;
use kosmos_hevy_worker::{
    annotate_workout, encode_query_value, page_count, page_items, workout_object, DataError,
    MAX_PAGE, MAX_RESPONSE_BYTES, ORIGIN, SYNC_KEY, TEMPLATES_PATH, WORKOUTS_PATH, WORKOUT_TYPE_ID,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
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
    secret_handles: BTreeMap<String, String>,
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
            DataError::Json => Self::Json,
            DataError::Invalid => Self::Invalid,
        }
    }
}

struct Client<'a> {
    input: io::Lines<io::StdinLock<'a>>,
    output: Arc<Mutex<BufWriter<io::Stdout>>>,
    token: String,
    generation: u64,
    secret_handle: String,
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
        let id = format!("hevy-{}", self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        Self::send(
            &self.output,
            json!({
                "method": "worker.call", "id": id, "generation": self.generation,
                "token": self.token, "operation": operation, "params": params
            }),
        )?;
        for line in self.input.by_ref() {
            let message: Value = serde_json::from_str(&line.map_err(|_| WorkerError::Io)?)
                .map_err(|_| WorkerError::Json)?;
            match message.get("method").and_then(Value::as_str) {
                Some("worker.stop") => return Err(WorkerError::Stopped),
                Some("worker.run") => continue,
                Some("worker.result") if message.get("id").and_then(Value::as_str) == Some(&id) => {
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

    fn fetch(&mut self, url: &str) -> Result<Value, WorkerError> {
        if !url
            .strip_prefix(ORIGIN)
            .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return Err(WorkerError::Invalid);
        }
        let result = self.call(
            "network.fetch",
            json!({"url": url, "secret_handle": self.secret_handle}),
        )?;
        let bytes = result
            .get("bytes")
            .and_then(Value::as_str)
            .and_then(|value| STANDARD.decode(value).ok())
            .ok_or(WorkerError::Host)?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(WorkerError::Invalid);
        }
        serde_json::from_slice(&bytes).map_err(|_| WorkerError::Json)
    }

    fn ark_write(&mut self, operation: &str, params: Value) -> Result<Value, WorkerError> {
        self.call(
            "ark.write",
            json!({"operation": operation, "params": params}),
        )
    }

    fn ark_read(&mut self, operation: &str, params: Value) -> Result<Value, WorkerError> {
        self.call("ark.read", json!({"operation":operation,"params":params}))
    }

    fn sync_value(&mut self) -> Result<Option<String>, WorkerError> {
        self.ark_read("get_sync_kv", json!({"key":SYNC_KEY}))
            .map(|value| value.as_str().map(str::to_owned))
    }
}

fn fetch_pages(
    client: &mut Client<'_>,
    path: &str,
    key: &str,
    size: u32,
    since: Option<&str>,
) -> Result<Vec<Value>, WorkerError> {
    let mut all = Vec::new();
    for page in 1..=MAX_PAGE {
        let suffix = since.map_or_else(String::new, |value| {
            format!("&since={}", encode_query_value(value))
        });
        let body = client.fetch(&format!(
            "{ORIGIN}{path}?page={page}&pageSize={size}{suffix}"
        ))?;
        let items = page_items(&body, key)?;
        if all.len().saturating_add(items.len()) > kosmos_hevy_worker::MAX_ITEMS {
            return Err(WorkerError::Invalid);
        }
        all.extend(items);
        if page >= page_count(&body) {
            return Ok(all);
        }
    }
    Err(WorkerError::Invalid)
}

fn sync(client: &mut Client<'_>) -> Result<(), WorkerError> {
    let last_success = client.sync_value()?;
    let templates = fetch_pages(client, TEMPLATES_PATH, "exercise_templates", 100, None)?
        .into_iter()
        .filter_map(|template| Some((template.get("id")?.as_str()?.to_owned(), template)))
        .collect::<HashMap<_, _>>();
    let mut deleted_ids = Vec::new();
    let workouts = if let Some(since) = last_success.as_deref() {
        let events = fetch_pages(client, "/v1/workouts/events", "events", 10, Some(since))?;
        let mut updated = Vec::new();
        for event in events {
            match event.get("type").and_then(Value::as_str) {
                Some("updated") => {
                    if let Some(workout) = event.get("workout") {
                        updated.push(workout.clone());
                    }
                }
                Some("deleted") => {
                    if let Some(id) = event.get("id").and_then(Value::as_str) {
                        deleted_ids.push(id.to_owned());
                    }
                }
                _ => {}
            }
        }
        updated
    } else {
        fetch_pages(client, WORKOUTS_PATH, "workouts", 10, None)?
    };
    let now = Utc::now().to_rfc3339();
    client.ark_write(
        "upsert_object_type",
        json!({"object_type": {
            "id": WORKOUT_TYPE_ID, "name": "Тренировка", "schemaJson": "{}", "uiSchemaJson": "{}",
            "createdAt": now, "updatedAt": now, "systemLocked": false
        }}),
    )?;
    for mut workout in workouts {
        annotate_workout(&mut workout, &templates);
        client.ark_write(
            "upsert_object",
            json!({"object": workout_object(&workout)?}),
        )?;
    }
    for id in deleted_ids {
        client.ark_write("delete_object", json!({"id":format!("hevy-workout:{id}")}))?;
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
    let secret_handle = bootstrap
        .integration
        .as_ref()
        .and_then(|integration| integration.secret_handles.get("api_key"))
        .filter(|handle| handle.len() == 64 && handle.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .cloned()
        .ok_or(WorkerError::Invalid)?;
    let output = Arc::new(Mutex::new(BufWriter::new(io::stdout())));
    Client::send(
        &output,
        json!({"method":"worker.hello","package_id":bootstrap.package_id,"version":bootstrap.version,
        "hash":bootstrap.hash,"pid":bootstrap.pid,"api_version":bootstrap.api_version,"token":bootstrap.token}),
    )?;
    let heartbeat_output = Arc::clone(&output);
    thread::spawn({
        let token = bootstrap.token.clone();
        let generation = bootstrap.generation;
        move || heartbeat(heartbeat_output, generation, token)
    });
    let mut client = Client {
        input,
        output,
        token: bootstrap.token,
        generation: bootstrap.generation,
        secret_handle,
        next_id: 0,
    };
    while let Some(message) = client.next_message()? {
        match message.get("method").and_then(Value::as_str) {
            Some("worker.stop") => return Ok(()),
            Some("worker.run")
                if message.get("generation").and_then(Value::as_u64) == Some(client.generation) =>
            {
                if let Err(error) = sync(&mut client) {
                    if matches!(error, WorkerError::Stopped) {
                        return Ok(());
                    }
                    eprintln!("Hevy sync failed");
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
