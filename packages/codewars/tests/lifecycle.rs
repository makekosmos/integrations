use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};

fn next_message(output: &mut BufReader<std::process::ChildStdout>) -> Value {
    loop {
        let mut line = String::new();
        output.read_line(&mut line).expect("worker output");
        let message: Value = serde_json::from_str(&line).expect("worker JSON");
        if message["method"] != "worker.heartbeat" {
            return message;
        }
    }
}

fn reply(input: &mut impl Write, call: &Value, result: Value) {
    writeln!(
        input,
        "{}",
        json!({"method":"worker.result","id":call["id"],"ok":true,"result":result,"error":null})
    )
    .expect("worker input");
    input.flush().expect("worker input flush");
}

#[test]
fn runs_mocked_profile_completion_and_rank_sync() {
    let mut worker = Command::new(env!("CARGO_BIN_EXE_codewars-worker"))
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
            "method":"worker.bootstrap","package_id":"com.kosmos.codewars","version":"0.1.0",
            "hash":"archive-hash","pid":42,"api_version":1,"generation":3,"token":"opaque-token",
            "integration":{"settings":[{"key":"username","kind":"text","required":true}],"values":{"username":"tester"},"secret_handles":{},"schedule":{"interval_seconds":86400}}
        })
    )
    .expect("bootstrap");
    input.flush().expect("bootstrap flush");
    let hello = next_message(&mut output);
    assert_eq!(hello["method"], "worker.hello");
    assert_eq!(hello["package_id"], "com.kosmos.codewars");
    writeln!(
        input,
        "{}",
        json!({"method":"worker.run","generation":3,"run_id":"run-1"})
    )
    .expect("run");
    input.flush().expect("run flush");

    let profile_call = next_message(&mut output);
    assert_eq!(profile_call["operation"], "network.fetch");
    assert_eq!(
        profile_call["params"]["url"],
        "https://www.codewars.com/api/v1/users/tester"
    );
    reply(
        &mut input,
        &profile_call,
        json!({"bytes":STANDARD.encode(r#"{"username":"tester","honor":544,"leaderboardPosition":134,"ranks":{"overall":{"name":"3 kyu"}},"codeChallenges":{"totalCompleted":1}}"#)}),
    );

    let existing = next_message(&mut output);
    assert_eq!(existing["operation"], "ark.read");
    reply(&mut input, &existing, json!([]));
    let state = next_message(&mut output);
    assert_eq!(state["operation"], "ark.read");
    reply(&mut input, &state, Value::Null);
    let page_call = next_message(&mut output);
    assert_eq!(
        page_call["params"]["url"],
        "https://www.codewars.com/api/v1/users/tester/code-challenges/completed?page=0"
    );
    reply(
        &mut input,
        &page_call,
        json!({"bytes":STANDARD.encode(r#"{"totalPages":1,"data":[{"id":"kata-1","name":"Sample","slug":"sample","completedAt":"2026-08-28T10:00:00Z","completedLanguages":["rust"]}]}"#)}),
    );

    let type_call = next_message(&mut output);
    assert_eq!(type_call["params"]["operation"], "upsert_object_type");
    reply(&mut input, &type_call, Value::Null);
    let second_type_call = next_message(&mut output);
    assert_eq!(
        second_type_call["params"]["operation"],
        "upsert_object_type"
    );
    reply(&mut input, &second_type_call, Value::Null);

    let profile_write = next_message(&mut output);
    assert_eq!(profile_write["params"]["operation"], "upsert_object");
    assert_eq!(
        profile_write["params"]["params"]["object"]["id"],
        "codewars-profile:current"
    );
    reply(&mut input, &profile_write, Value::Null);

    let rank_call = next_message(&mut output);
    assert_eq!(
        rank_call["params"]["url"],
        "https://www.codewars.com/api/v1/code-challenges/kata-1"
    );
    reply(
        &mut input,
        &rank_call,
        json!({"bytes":STANDARD.encode(r#"{"rank":{"name":"6 kyu"}}"#)}),
    );
    let completion_write = next_message(&mut output);
    assert_eq!(
        completion_write["params"]["params"]["object"]["id"],
        "codewars-completion:tester:kata-1"
    );
    assert_eq!(
        completion_write["params"]["params"]["object"]["propsJson"]["rank"]["name"],
        "6 kyu"
    );
    reply(&mut input, &completion_write, Value::Null);

    writeln!(input, "{}", json!({"method":"worker.stop","generation":3})).expect("stop");
    input.flush().expect("stop flush");
    assert!(worker.wait().expect("worker wait").success());
}
