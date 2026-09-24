use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};

fn next_message(output: &mut BufReader<std::process::ChildStdout>) -> Value {
    let mut line = String::new();
    output.read_line(&mut line).expect("worker output");
    serde_json::from_str(&line).expect("worker JSON")
}

fn reply(input: &mut impl Write, call: &Value, result: Value) {
    writeln!(
        input,
        "{}",
        json!({
            "method": "worker.result",
            "id": call["id"],
            "ok": true,
            "result": result,
            "error": null
        })
    )
    .expect("worker input");
    input.flush().expect("worker input flush");
}

#[test]
fn runs_mock_sync_with_opaque_cookie_handle_and_mapping() {
    let mut worker = Command::new(env!("CARGO_BIN_EXE_greatfrontend-worker"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("worker process");
    let mut input = worker.stdin.take().expect("worker stdin");
    let mut output = BufReader::new(worker.stdout.take().expect("worker stdout"));
    let handle = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    writeln!(
        input,
        "{}",
        json!({
            "method": "worker.bootstrap",
            "package_id": "com.kosmos.greatfrontend",
            "version": "0.1.0",
            "hash": "archive-hash",
            "pid": 42,
            "api_version": 1,
            "generation": 3,
            "correlation_id": "test",
            "token": "opaque-token",
            "integration": {
                "settings": [{
                    "key": "session",
                    "label": "Session",
                    "kind": "secret",
                    "required": true,
                    "injection": {"kind":"cookies","origins":["https://www.greatfrontend.com/"]}
                }],
                "values": {},
                "secret_handles": {"session": handle},
                "schedule": {"interval_seconds": 86400}
            }
        })
    )
    .expect("bootstrap");
    input.flush().expect("bootstrap flush");

    let hello = next_message(&mut output);
    assert_eq!(hello["method"], "worker.hello");
    assert_eq!(hello["package_id"], "com.kosmos.greatfrontend");

    writeln!(
        input,
        "{}",
        json!({"method":"worker.run","generation":3,"run_id":"run-1"})
    )
    .expect("run");
    input.flush().expect("run flush");

    let state_call = next_message(&mut output);
    assert_eq!(state_call["operation"], "ark.read");
    reply(&mut input, &state_call, Value::Null);
    let fetch_call = next_message(&mut output);
    assert_eq!(fetch_call["operation"], "network.fetch");
    assert_eq!(fetch_call["params"]["secret_handle"], handle);
    assert!(!fetch_call.to_string().contains("secret-value"));
    assert_eq!(
        fetch_call["params"]["url"],
        "https://www.greatfrontend.com/api/trpc/questionProgress.getAllIncludingMetadata?batch=1&input=%7B%220%22%3A%7B%22json%22%3Anull%2C%22meta%22%3A%7B%22values%22%3A%5B%22undefined%22%5D%2C%22v%22%3A1%7D%7D%7D"
    );
    reply(
        &mut input,
        &fetch_call,
        json!({"bytes": STANDARD.encode(r#"{"0":{"result":{"data":{"json":[{"id":"p1","createdAt":"2026-08-28T10:00:00Z","metadata":{"title":"Binary Search","slug":"binary-search","format":"question","href":"/questions/binary-search"}}]}}}}"#)}),
    );

    let type_call = next_message(&mut output);
    assert_eq!(type_call["operation"], "ark.write");
    assert_eq!(type_call["params"]["operation"], "upsert_object_type");
    reply(&mut input, &type_call, Value::Null);

    let object_call = next_message(&mut output);
    assert_eq!(object_call["params"]["operation"], "upsert_object");
    assert_eq!(
        object_call["params"]["params"]["object"]["id"],
        "greatfrontend-completion:p1"
    );
    assert_eq!(
        object_call["params"]["params"]["object"]["propsJson"]["url"],
        "https://www.greatfrontend.com/questions/binary-search"
    );
    reply(&mut input, &object_call, Value::Null);

    writeln!(
        input,
        "{}",
        json!({"method":"worker.stop","generation":3,"reason":"test"})
    )
    .expect("stop");
    input.flush().expect("stop flush");
    assert!(worker.wait().expect("worker wait").success());
}
