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
fn runs_host_triggered_sync_and_writes_mapped_completion() {
    let mut worker = Command::new(env!("CARGO_BIN_EXE_bigfrontend-worker"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("worker process");
    let mut input = worker.stdin.take().expect("worker stdin");
    let mut output = BufReader::new(worker.stdout.take().expect("worker stdout"));
    writeln!(
        input,
        "{}",
        json!({
            "method": "worker.bootstrap",
            "package_id": "com.kosmos.bigfrontend",
            "version": "0.1.0",
            "hash": "archive-hash",
            "pid": 42,
            "api_version": 1,
            "generation": 3,
            "correlation_id": "test",
            "token": "opaque-token",
            "integration": {
                "settings": [{"key":"username","label":"Username","kind":"text","required":true}],
                "values": {"username":"public-user"},
                "secret_handles": {},
                "schedule": {"interval_seconds": 86400}
            }
        })
    )
    .expect("bootstrap");
    input.flush().expect("bootstrap flush");

    let hello = next_message(&mut output);
    assert_eq!(hello["method"], "worker.hello");
    assert_eq!(hello["package_id"], "com.kosmos.bigfrontend");

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
    let profile_call = next_message(&mut output);
    assert_eq!(profile_call["operation"], "network.fetch");
    assert_eq!(
        profile_call["params"]["url"],
        "https://bigfrontend.dev/user/public-user"
    );
    reply(
        &mut input,
        &profile_call,
        json!({"bytes": STANDARD.encode(r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"profile":{"id":42}}}}</script>"#)}),
    );

    let activity_call = next_message(&mut output);
    assert_eq!(
        activity_call["params"]["url"],
        "https://bigfrontend.dev/api/activity?type=submission&userId=42"
    );
    reply(
        &mut input,
        &activity_call,
        json!({"bytes": STANDARD.encode(r#"{"items":[{"id":7,"createdAt":"2026-08-28T10:00:00Z","target":{"title":"Memoize","permalink":"memoize","targetType":"problem"}}]}"#)}),
    );

    let type_call = next_message(&mut output);
    assert_eq!(type_call["operation"], "ark.write");
    assert_eq!(type_call["params"]["operation"], "upsert_object_type");
    reply(&mut input, &type_call, Value::Null);

    let object_call = next_message(&mut output);
    assert_eq!(object_call["params"]["operation"], "upsert_object");
    assert_eq!(
        object_call["params"]["params"]["object"]["id"],
        "bigfrontend-completion:7"
    );
    assert_eq!(
        object_call["params"]["params"]["object"]["propsJson"]["url"],
        "https://bigfrontend.dev/problem/memoize"
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
