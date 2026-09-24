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
            "method":"worker.result", "id":call["id"], "ok":true, "result":result, "error":null
        })
    )
    .expect("worker input");
    input.flush().expect("worker input flush");
}

fn graphql_response(value: Value) -> Value {
    json!({"bytes": STANDARD.encode(value.to_string())})
}

#[test]
fn runs_authenticated_graphql_lifecycle_with_only_opaque_handle() {
    let mut worker = Command::new(env!("CARGO_BIN_EXE_leetcode-worker"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("worker process");
    let mut input = worker.stdin.take().expect("worker stdin");
    let mut output = BufReader::new(worker.stdout.take().expect("worker stdout"));
    writeln!(input, "{}", json!({
        "method":"worker.bootstrap", "package_id":"com.kosmos.leetcode", "version":"0.1.0",
        "hash":"archive-hash", "pid":42, "api_version":1, "generation":3,
        "correlation_id":"test", "token":"opaque-token", "integration":{
            "settings":[{"key":"session","label":"Session","kind":"secret","required":true}],
            "values":{}, "secret_handles":{"session":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"},
            "schedule":{"interval_seconds":86400}
        }
    }))
    .expect("bootstrap");
    input.flush().expect("bootstrap flush");
    assert_eq!(next_message(&mut output)["method"], "worker.hello");
    writeln!(
        input,
        "{}",
        json!({"method":"worker.run","generation":3,"run_id":"run-1"})
    )
    .expect("run");
    input.flush().expect("run flush");

    let existing = next_message(&mut output);
    assert_eq!(existing["operation"], "ark.read");
    reply(&mut input, &existing, json!([]));
    let state = next_message(&mut output);
    assert_eq!(state["operation"], "ark.read");
    reply(&mut input, &state, Value::Null);
    let global = next_message(&mut output);
    assert_eq!(global["operation"], "network.fetch");
    assert_eq!(global["params"]["url"], "https://leetcode.com/graphql");
    assert_eq!(global["params"]["body"]["variables"], json!({}));
    assert!(global["params"]["body"]["query"]
        .as_str()
        .unwrap()
        .contains("globalData"));
    assert_eq!(
        global["params"]["secret_handle"],
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    );
    assert!(!global["params"]["url"]
        .as_str()
        .unwrap()
        .contains("LEETCODE_SESSION"));
    reply(
        &mut input,
        &global,
        graphql_response(json!({"data":{"userStatus":{"isSignedIn":true,"username":"tester"}}})),
    );

    let profile = next_message(&mut output);
    assert_eq!(profile["operation"], "network.fetch");
    reply(
        &mut input,
        &profile,
        graphql_response(
            json!({"data":{"matchedUser":{"submitStatsGlobal":{"acSubmissionNum":[{"difficulty":"All","count":2},{"difficulty":"Easy","count":1},{"difficulty":"Medium","count":1},{"difficulty":"Hard","count":0}]}},"allQuestionsCount":[{"difficulty":"All","count":3},{"difficulty":"Easy","count":2},{"difficulty":"Medium","count":1},{"difficulty":"Hard","count":0}]}}),
        ),
    );

    let submissions = next_message(&mut output);
    assert_eq!(submissions["operation"], "network.fetch");
    reply(
        &mut input,
        &submissions,
        graphql_response(
            json!({"data":{"submissionList":{"lastKey":null,"hasNext":false,"submissions":[{"id":"42","title":"Two Sum","titleSlug":"two-sum","statusDisplay":"Accepted","lang":"rust","timestamp":"1775606400","url":"/submissions/detail/42/","memory":"1 MB","runtime":"1 ms"}]}}}),
        ),
    );

    let questions = next_message(&mut output);
    assert_eq!(questions["operation"], "network.fetch");
    reply(
        &mut input,
        &questions,
        graphql_response(json!({"data":{"q0":{"questionFrontendId":"1"}}})),
    );

    for expected in [
        "upsert_object_type",
        "upsert_object_type",
        "upsert_object",
        "upsert_object",
    ] {
        let call = next_message(&mut output);
        assert_eq!(call["operation"], "ark.write");
        assert_eq!(call["params"]["operation"], expected);
        if expected == "upsert_object"
            && call["params"]["params"]["object"]["typeId"] == "coding_submission_obj"
        {
            assert_eq!(
                call["params"]["params"]["object"]["propsJson"]["problemNumber"],
                "1"
            );
        }
        reply(&mut input, &call, Value::Null);
    }
    writeln!(
        input,
        "{}",
        json!({"method":"worker.stop","generation":3,"reason":"test"})
    )
    .expect("stop");
    input.flush().expect("stop flush");
    assert!(worker.wait().expect("worker wait").success());
}
