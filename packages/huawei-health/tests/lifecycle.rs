use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const ACCOUNT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const HANDLE: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

#[test]
fn bootstraps_with_opaque_session_and_account_key() {
    let mut worker = Command::new(env!("CARGO_BIN_EXE_huawei-health-worker"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = worker.stdin.take().unwrap();
    let mut output = BufReader::new(worker.stdout.take().unwrap());
    writeln!(
        input,
        "{}",
        json!({
            "method":"worker.bootstrap", "package_id":"com.kosmos.huawei-health",
            "version":"0.1.0", "hash":"h", "pid":42, "api_version":1,
            "generation":3, "correlation_id":"c", "token":"worker-token",
            "integration": {
                "account_key": ACCOUNT,
                "data_origin": "sportdata-dre.things.dbankcloud.com",
                "site_id": 7,
                "settings": [{"key":"session","label":"Session","kind":"secret","required":true}],
                "values": {}, "secret_handles": {"session":HANDLE}
            }
        })
    )
    .unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let hello: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(hello["method"], "worker.hello");
    assert!(!hello.to_string().contains(HANDLE));
    writeln!(
        input,
        "{}",
        json!({"method":"worker.stop","generation":3,"reason":"test"})
    )
    .unwrap();
    input.flush().unwrap();
    assert!(worker.wait().unwrap().success());
}

#[test]
fn stop_during_first_sync_call_exits_promptly() {
    let mut worker = Command::new(env!("CARGO_BIN_EXE_huawei-health-worker"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = worker.stdin.take().unwrap();
    let mut output = BufReader::new(worker.stdout.take().unwrap());
    writeln!(
        input,
        "{}",
        json!({
            "method":"worker.bootstrap", "package_id":"com.kosmos.huawei-health",
            "version":"0.1.0", "hash":"h", "pid":42, "api_version":1,
            "generation":3, "token":"worker-token",
            "integration": {
                "account_key": ACCOUNT,
                "data_origin": "sportdata-dre.things.dbankcloud.com",
                "site_id": 7,
                "settings": [{"key":"session","label":"Session","kind":"secret","required":true}],
                "values": {}, "secret_handles": {"session":HANDLE}
            }
        })
    )
    .unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap()["method"],
        "worker.hello"
    );
    writeln!(
        input,
        "{}",
        json!({"method":"worker.run","generation":3,"run_id":"run-1"})
    )
    .unwrap();
    input.flush().unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    let call: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(call["method"], "worker.call");
    assert_eq!(call["operation"], "ark.write");
    writeln!(
        input,
        "{}",
        json!({"method":"worker.result","id":call["id"],"generation":3,"ok":true,"result":null})
    )
    .unwrap();
    writeln!(input, "{}", json!({"method":"worker.stop","generation":3})).unwrap();
    input.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = worker.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= deadline {
            let _ = worker.kill();
            panic!("worker did not stop during an in-flight call");
        }
        thread::sleep(Duration::from_millis(10));
    }
}
