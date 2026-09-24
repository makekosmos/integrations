use kosmos_huawei_health_worker::{Archive, Error as ArchiveError, DATA_ORIGINS};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufWriter, Write},
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
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
    #[serde(default)]
    account_key: Option<String>,
    data_origin: String,
    site_id: u32,
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

type Incoming = Result<Value, WorkerError>;

struct Client {
    input: Receiver<Incoming>,
    output: Arc<Mutex<BufWriter<io::Stdout>>>,
    token: String,
    generation: u64,
    next_id: u64,
}

impl Client {
    fn next_message(&mut self) -> Result<Option<Value>, WorkerError> {
        match self.input.recv() {
            Ok(message) => message.map(Some),
            Err(_) => Ok(None),
        }
    }

    fn send(output: &Arc<Mutex<BufWriter<io::Stdout>>>, message: Value) -> Result<(), WorkerError> {
        let mut output = output.lock().map_err(|_| WorkerError::Io)?;
        serde_json::to_writer(&mut *output, &message).map_err(|_| WorkerError::Json)?;
        output.write_all(b"\n").map_err(|_| WorkerError::Io)?;
        output.flush().map_err(|_| WorkerError::Io)
    }

    fn call(&mut self, operation: &str, params: Value) -> Result<Value, WorkerError> {
        let id = format!("huawei-health-{}", self.next_id);
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
        loop {
            let message = self.input.recv().map_err(|_| WorkerError::Io)??;
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
    }

    fn wait_retry(&mut self, delay: Duration) -> Result<(), WorkerError> {
        let deadline = Instant::now() + delay;
        loop {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Ok(());
            };
            match self.input.recv_timeout(remaining) {
                Ok(Ok(message))
                    if message.get("method").and_then(Value::as_str) == Some("worker.stop") =>
                {
                    return Err(WorkerError::Stopped);
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => return Err(error),
                Err(RecvTimeoutError::Timeout) => return Ok(()),
                Err(RecvTimeoutError::Disconnected) => return Err(WorkerError::Io),
            }
        }
    }
}

fn map_worker_error(error: WorkerError) -> ArchiveError {
    match error {
        WorkerError::Stopped => ArchiveError::Stopped,
        WorkerError::Host => ArchiveError::Host,
        WorkerError::Io | WorkerError::Json | WorkerError::Invalid => ArchiveError::Invalid,
    }
}

fn map_archive_error(error: ArchiveError) -> WorkerError {
    match error {
        ArchiveError::Stopped => WorkerError::Stopped,
        ArchiveError::Host => WorkerError::Host,
        ArchiveError::Invalid => WorkerError::Invalid,
    }
}

fn run_archive<F, W>(archive: &mut Archive<F, W>) -> Result<(), WorkerError>
where
    F: FnMut(&str, Value) -> kosmos_huawei_health_worker::Result<Value>,
    W: FnMut(Duration) -> kosmos_huawei_health_worker::Result<()>,
{
    archive.sync().map_err(map_archive_error)
}

fn valid_handle(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sync(
    client: std::rc::Rc<std::cell::RefCell<Client>>,
    account: String,
    handle: String,
    data_origin: String,
    site_id: u32,
) -> Result<(), WorkerError> {
    let call_client = std::rc::Rc::clone(&client);
    let wait_client = std::rc::Rc::clone(&client);
    let mut archive = Archive {
        account,
        handle,
        data_origin,
        site_id,
        call: move |operation: &str, params: Value| -> kosmos_huawei_health_worker::Result<Value> {
            call_client
                .borrow_mut()
                .call(operation, params)
                .map_err(map_worker_error)
        },
        wait: move |delay: Duration| -> kosmos_huawei_health_worker::Result<()> {
            wait_client
                .borrow_mut()
                .wait_retry(delay)
                .map_err(map_worker_error)
        },
    };
    run_archive(&mut archive)
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

fn spawn_reader() -> Receiver<Incoming> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let message = line
                .map_err(|_| WorkerError::Io)
                .and_then(|line| serde_json::from_str(&line).map_err(|_| WorkerError::Json));
            if sender.send(message).is_err() {
                return;
            }
        }
    });
    receiver
}

fn run() -> Result<(), WorkerError> {
    let input = spawn_reader();
    let bootstrap = match input.recv().map_err(|_| WorkerError::Io)?? {
        message if message.get("method").and_then(Value::as_str) == Some("worker.bootstrap") => {
            serde_json::from_value::<Bootstrap>(message).map_err(|_| WorkerError::Json)?
        }
        _ => return Err(WorkerError::Invalid),
    };
    let integration = bootstrap.integration.as_ref().ok_or(WorkerError::Invalid)?;
    let account = integration
        .account_key
        .as_deref()
        .filter(|key| valid_handle(key))
        .ok_or(WorkerError::Invalid)?
        .to_owned();
    if !DATA_ORIGINS.contains(&integration.data_origin.as_str()) || integration.site_id == 0 {
        return Err(WorkerError::Invalid);
    }
    let handle = integration
        .secret_handles
        .get("session")
        .filter(|handle| valid_handle(handle))
        .cloned()
        .ok_or(WorkerError::Invalid)?;
    let output = Arc::new(Mutex::new(BufWriter::new(io::stdout())));
    Client::send(
        &output,
        json!({
            "method": "worker.hello",
            "package_id": bootstrap.package_id,
            "version": bootstrap.version,
            "hash": bootstrap.hash,
            "pid": bootstrap.pid,
            "api_version": bootstrap.api_version,
            "token": bootstrap.token
        }),
    )?;
    let heartbeat_output = Arc::clone(&output);
    let heartbeat_token = bootstrap.token.clone();
    thread::spawn(move || heartbeat(heartbeat_output, bootstrap.generation, heartbeat_token));
    let generation = bootstrap.generation;
    let token = bootstrap.token;
    let data_origin = integration.data_origin.clone();
    let site_id = integration.site_id;
    let client = std::rc::Rc::new(std::cell::RefCell::new(Client {
        input,
        output,
        token,
        generation,
        next_id: 0,
    }));
    loop {
        let message = { client.borrow_mut().next_message()? };
        let Some(message) = message else {
            return Ok(());
        };
        match message.get("method").and_then(Value::as_str) {
            Some("worker.stop") => return Ok(()),
            Some("worker.run")
                if message.get("generation").and_then(Value::as_u64) == Some(generation) =>
            {
                match sync(
                    std::rc::Rc::clone(&client),
                    account.clone(),
                    handle.clone(),
                    data_origin.clone(),
                    site_id,
                ) {
                    Ok(()) => {}
                    Err(WorkerError::Stopped) => return Ok(()),
                    Err(error) => return Err(error),
                }
            }
            _ => {}
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Huawei Health worker failed: {error:?}");
        std::process::exit(1);
    }
}
