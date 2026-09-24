use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};

fn next(output: &mut BufReader<std::process::ChildStdout>) -> Value {
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn reply(input: &mut impl Write, call: &Value, result: Value) {
    writeln!(
        input,
        "{}",
        json!({"method":"worker.result","id":call["id"],"ok":true,"result":result,"error":null})
    )
    .unwrap();
    input.flush().unwrap();
}

#[test]
fn uses_only_opaque_handle_and_writes_time_entry() {
    let mut worker = Command::new(env!("CARGO_BIN_EXE_toggl-worker"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = worker.stdin.take().unwrap();
    let mut output = BufReader::new(worker.stdout.take().unwrap());
    let handle = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
    writeln!(input, "{}", json!({"method":"worker.bootstrap","package_id":"com.kosmos.toggl","version":"0.1.0","hash":"h","pid":42,"api_version":1,"generation":3,"correlation_id":"c","token":"t","integration":{"settings":[{"key":"api_token","label":"API token","kind":"secret","required":true,"injection":{"kind":"basic","origins":["https://api.track.toggl.com/"],"password":"api_token"}}],"values":{},"secret_handles":{"api_token":handle},"schedule":{"interval_seconds":3600}}})).unwrap();
    input.flush().unwrap();
    assert_eq!(next(&mut output)["method"], "worker.hello");
    writeln!(
        input,
        "{}",
        json!({"method":"worker.run","generation":3,"run_id":"run-1"})
    )
    .unwrap();
    input.flush().unwrap();
    let state = next(&mut output);
    assert_eq!(state["operation"], "ark.read");
    reply(&mut input, &state, Value::Null);
    let page = next(&mut output);
    assert_eq!(
        page["params"]["url"],
        "https://api.track.toggl.com/api/v9/me/time_entries"
    );
    assert_eq!(page["params"]["secret_handle"], handle);
    assert!(!page.to_string().contains("secret-value"));
    reply(
        &mut input,
        &page,
        json!({"bytes":STANDARD.encode(r#"[{"id":42,"description":"Project","start":"2026-07-17T10:00:00Z","stop":null,"duration":-1,"at":"2026-07-17T10:00:00Z","workspace_id":7}]"#)}),
    );
    let empty_page = next(&mut output);
    assert_eq!(
        empty_page["params"]["url"],
        "https://api.track.toggl.com/api/v9/me/time_entries?before=2026-07-17T09%3A59%3A59.999%2B00%3A00"
    );
    reply(
        &mut input,
        &empty_page,
        json!({"bytes":STANDARD.encode("[]")}),
    );
    let type_call = next(&mut output);
    assert_eq!(type_call["params"]["operation"], "upsert_object_type");
    reply(&mut input, &type_call, Value::Null);
    let object = next(&mut output);
    assert_eq!(object["params"]["operation"], "upsert_object");
    assert_eq!(
        object["params"]["params"]["object"]["id"],
        "toggl-time-entry:42"
    );
    reply(&mut input, &object, Value::Null);
    writeln!(
        input,
        "{}",
        json!({"method":"worker.stop","generation":3,"reason":"test"})
    )
    .unwrap();
    input.flush().unwrap();
    assert!(worker.wait().unwrap().success());
}
