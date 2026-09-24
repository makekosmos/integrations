use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};

fn next(output: &mut BufReader<std::process::ChildStdout>) -> Value {
    let mut line = String::new();
    output.read_line(&mut line).expect("worker output");
    serde_json::from_str(&line).expect("worker JSON")
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
fn uses_only_opaque_handle_and_writes_workout() {
    let mut worker = Command::new(env!("CARGO_BIN_EXE_hevy-worker"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = worker.stdin.take().unwrap();
    let mut output = BufReader::new(worker.stdout.take().unwrap());
    let handle = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    writeln!(input, "{}", json!({"method":"worker.bootstrap","package_id":"com.kosmos.hevy","version":"0.1.0","hash":"h","pid":42,"api_version":1,"generation":3,"correlation_id":"c","token":"t","integration":{"settings":[{"key":"api_key","label":"API key","kind":"secret","required":true,"injection":{"kind":"header","origins":["https://api.hevyapp.com/"],"name":"api-key"}}],"values":{},"secret_handles":{"api_key":handle},"schedule":{"interval_seconds":21600}}})).unwrap();
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
    let templates = next(&mut output);
    assert_eq!(
        templates["params"]["url"],
        "https://api.hevyapp.com/v1/exercise_templates?page=1&pageSize=100"
    );
    assert_eq!(templates["params"]["secret_handle"], handle);
    assert!(templates.to_string().find("secret-value").is_none());
    reply(
        &mut input,
        &templates,
        json!({"bytes":STANDARD.encode(r#"{"page_count":1,"exercise_templates":[{"id":"bench","primary_muscle_group":"chest"}]}"#)}),
    );
    let workouts = next(&mut output);
    assert_eq!(
        workouts["params"]["url"],
        "https://api.hevyapp.com/v1/workouts?page=1&pageSize=10"
    );
    reply(
        &mut input,
        &workouts,
        json!({"bytes":STANDARD.encode(r#"{"page_count":1,"workouts":[{"id":"w1","title":"Push","exercises":[{"exercise_template_id":"bench"}]}]}"#)}),
    );
    let type_call = next(&mut output);
    assert_eq!(type_call["params"]["operation"], "upsert_object_type");
    reply(&mut input, &type_call, Value::Null);
    let object = next(&mut output);
    assert_eq!(object["params"]["operation"], "upsert_object");
    assert_eq!(
        object["params"]["params"]["object"]["id"],
        "hevy-workout:w1"
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
