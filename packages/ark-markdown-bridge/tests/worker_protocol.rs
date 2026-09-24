#![allow(clippy::unwrap_used)]

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    time::Duration,
};

fn reply(stdin: &mut impl Write, call: &Value, ok: bool, result: Value) {
    writeln!(
        stdin,
        "{}",
        json!({"method":"worker.result","id":call["id"],"ok":ok,"result":result})
    )
    .unwrap();
    stdin.flush().unwrap();
}
fn next(reader: &mut BufReader<impl std::io::Read>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}
fn call(reader: &mut BufReader<impl std::io::Read>) -> Value {
    loop {
        let value = next(reader);
        if value["method"] == "worker.call" {
            return value;
        }
    }
}

#[test]
fn worker_projects_and_imports_only_through_broker_stdio() {
    let temp = tempfile::tempdir().unwrap();
    let vault = temp.path().join("vault");
    let state_root = temp.path().join("state");
    std::fs::create_dir(&vault).unwrap();
    std::fs::create_dir(&state_root).unwrap();
    let vault_root = vault.to_string_lossy().into_owned();
    let state_root = state_root.to_string_lossy().into_owned();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ark-markdown-bridge"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    writeln!(input, "{}", json!({"method":"worker.bootstrap","package_id":"ark-markdown-bridge","version":"1.0.0","hash":"a","pid":1,"api_version":1,"generation":1,"correlation_id":"test","token":"token","bridge_config":{"vault_root":vault_root,"state_root":state_root,"selected_types":["com.kosmos.note"],"editable_fields":["title","body"],"readonly_fields":[]}})).unwrap();
    input.flush().unwrap();
    assert_eq!(next(&mut output)["method"], "worker.hello");
    let object = json!({"id":"note-1","type_id":"com.kosmos.note","type_version":"1.0.0","title":"One","props_json":{"description":null,"extensions":{}},"content_json":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"From ARK"}]}]},"created_at":"x","updated_at":"x","deleted_at":null});
    let state_read = call(&mut output);
    assert_eq!(state_read["operation"], "filesystem.read");
    reply(&mut input, &state_read, false, Value::Null);
    let poll = call(&mut output);
    assert_eq!(poll["operation"], "filesystem.poll");
    reply(&mut input, &poll, true, json!([]));
    let list = call(&mut output);
    assert_eq!(list["operation"], "filesystem.list");
    reply(&mut input, &list, true, json!([]));
    let ark = call(&mut output);
    assert_eq!(ark["operation"], "ark.read");
    assert_eq!(ark["params"]["operation"], "list_objects");
    reply(&mut input, &ark, true, json!([object]));
    let compare = call(&mut output);
    assert_eq!(compare["operation"], "filesystem.read");
    reply(&mut input, &compare, false, Value::Null);
    let projection = call(&mut output);
    assert_eq!(projection["operation"], "filesystem.write");
    assert_eq!(
        projection["params"]["path"],
        format!("{}{}One-note-1.md", vault_root, std::path::MAIN_SEPARATOR)
    );
    let markdown = String::from_utf8(
        STANDARD
            .decode(projection["params"]["bytes"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert!(markdown.contains("ark_id: \"note-1\""));
    reply(&mut input, &projection, true, Value::Null);
    let provenance = call(&mut output);
    assert_eq!(provenance["operation"], "ark.write");
    assert_eq!(provenance["params"]["operation"], "external_refs.upsert");
    assert_eq!(provenance["params"]["params"]["state"], "clean");
    reply(&mut input, &provenance, true, Value::Null);
    let state_write = call(&mut output);
    assert_eq!(state_write["operation"], "filesystem.write");
    assert_eq!(
        state_write["params"]["path"],
        format!("{}{}state.json", state_root, std::path::MAIN_SEPARATOR)
    );
    let state = String::from_utf8(
        STANDARD
            .decode(state_write["params"]["bytes"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    reply(&mut input, &state_write, true, Value::Null);

    writeln!(input, "{}", json!({"method":"worker.tick"})).unwrap();
    input.flush().unwrap();
    let state_read = call(&mut output);
    reply(
        &mut input,
        &state_read,
        true,
        json!({"bytes":STANDARD.encode(state.as_bytes())}),
    );
    let poll = call(&mut output);
    reply(
        &mut input,
        &poll,
        true,
        json!([{ "name":"One-note-1.md", "kind":"file", "size": 80, "modified_ms": 1 }]),
    );
    let list = call(&mut output);
    reply(
        &mut input,
        &list,
        true,
        json!([{ "name":"One-note-1.md", "kind":"file", "size": 80, "modified_ms": 1 }]),
    );
    let file = call(&mut output);
    assert_eq!(file["operation"], "filesystem.read");
    let edited = markdown.replace("From ARK", "From vault");
    reply(
        &mut input,
        &file,
        true,
        json!({"bytes":STANDARD.encode(edited.as_bytes())}),
    );
    let ark = call(&mut output);
    assert_eq!(ark["operation"], "ark.read");
    reply(&mut input, &ark, true, json!([object]));
    let upsert = call(&mut output);
    assert_eq!(upsert["operation"], "ark.write");
    assert_eq!(upsert["params"]["operation"], "upsert_object");
    assert_eq!(
        upsert["params"]["params"]["object"]["content_json"]["content"][0]["content"][0]["text"],
        "From vault"
    );
    reply(&mut input, &upsert, true, Value::Null);
    let get = call(&mut output);
    assert_eq!(get["operation"], "ark.read");
    assert_eq!(get["params"]["operation"], "get_object");
    let updated = json!({"id":"note-1","type_id":"com.kosmos.note","type_version":"1.0.0","title":"One","props_json":{"description":null,"extensions":{}},"content_json":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"From vault"}]}]},"created_at":"x","updated_at":"y","deleted_at":null});
    reply(&mut input, &get, true, updated.clone());
    let provenance = call(&mut output);
    assert_eq!(provenance["operation"], "ark.write");
    reply(&mut input, &provenance, true, Value::Null);
    let state_write = call(&mut output);
    assert_eq!(state_write["operation"], "filesystem.write");
    let edited_state = String::from_utf8(
        STANDARD
            .decode(state_write["params"]["bytes"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    reply(&mut input, &state_write, true, Value::Null);

    // A later ARK-only edit updates the Markdown file through the same broker.
    writeln!(input, "{}", json!({"method":"worker.tick"})).unwrap();
    input.flush().unwrap();
    let state_read = call(&mut output);
    reply(
        &mut input,
        &state_read,
        true,
        json!({"bytes":STANDARD.encode(edited_state.as_bytes())}),
    );
    let poll = call(&mut output);
    reply(
        &mut input,
        &poll,
        true,
        json!([{ "name":"One-note-1.md", "kind":"file", "size": 80, "modified_ms": 2 }]),
    );
    let list = call(&mut output);
    reply(
        &mut input,
        &list,
        true,
        json!([{ "name":"One-note-1.md", "kind":"file", "size": 80, "modified_ms": 2 }]),
    );
    let file = call(&mut output);
    reply(
        &mut input,
        &file,
        true,
        json!({"bytes":STANDARD.encode(markdown.replace("From ARK", "From vault").as_bytes())}),
    );
    let ark = call(&mut output);
    let server = json!({"id":"note-1","type_id":"com.kosmos.note","type_version":"1.0.0","title":"One","props_json":{"description":null,"extensions":{}},"content_json":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"From server"}]}]},"created_at":"x","updated_at":"server","deleted_at":null});
    reply(&mut input, &ark, true, json!([server]));
    let compare = call(&mut output);
    assert_eq!(compare["operation"], "filesystem.read");
    reply(
        &mut input,
        &compare,
        true,
        json!({"bytes":STANDARD.encode(markdown.replace("From ARK", "From vault").as_bytes())}),
    );
    let server_write = call(&mut output);
    assert_eq!(server_write["operation"], "filesystem.write");
    let server_markdown = String::from_utf8(
        STANDARD
            .decode(server_write["params"]["bytes"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert!(server_markdown.contains("From server"));
    reply(&mut input, &server_write, true, Value::Null);
    let provenance = call(&mut output);
    assert_eq!(provenance["operation"], "ark.write");
    reply(&mut input, &provenance, true, Value::Null);
    let state_write = call(&mut output);
    let server_state = String::from_utf8(
        STANDARD
            .decode(state_write["params"]["bytes"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    reply(&mut input, &state_write, true, Value::Null);

    // Rename keeps ark_id and changes only private projection state.
    writeln!(input, "{}", json!({"method":"worker.tick"})).unwrap();
    input.flush().unwrap();
    let state_read = call(&mut output);
    reply(
        &mut input,
        &state_read,
        true,
        json!({"bytes":STANDARD.encode(server_state.as_bytes())}),
    );
    let poll = call(&mut output);
    reply(
        &mut input,
        &poll,
        true,
        json!([{ "name":"Renamed.md", "kind":"file", "size": 80, "modified_ms": 2 }]),
    );
    let list = call(&mut output);
    reply(
        &mut input,
        &list,
        true,
        json!([{ "name":"Renamed.md", "kind":"file", "size": 80, "modified_ms": 2 }]),
    );
    let file = call(&mut output);
    reply(
        &mut input,
        &file,
        true,
        json!({"bytes":STANDARD.encode(server_markdown.as_bytes())}),
    );
    let ark = call(&mut output);
    reply(&mut input, &ark, true, json!([server]));
    let provenance = call(&mut output);
    assert_eq!(provenance["operation"], "ark.write");
    reply(&mut input, &provenance, true, Value::Null);
    let state_write = call(&mut output);
    let renamed_state = String::from_utf8(
        STANDARD
            .decode(state_write["params"]["bytes"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert!(renamed_state.contains("Renamed.md"));
    reply(&mut input, &state_write, true, Value::Null);

    // A bridge-owned file whose ARK object disappeared is safely removed.
    writeln!(input, "{}", json!({"method":"worker.tick"})).unwrap();
    input.flush().unwrap();
    let state_read = call(&mut output);
    reply(
        &mut input,
        &state_read,
        true,
        json!({"bytes":STANDARD.encode(renamed_state.as_bytes())}),
    );
    let poll = call(&mut output);
    reply(
        &mut input,
        &poll,
        true,
        json!([{ "name":"Renamed.md", "kind":"file", "size": 80, "modified_ms": 6 }]),
    );
    let list = call(&mut output);
    reply(
        &mut input,
        &list,
        true,
        json!([{ "name":"Renamed.md", "kind":"file", "size": 80, "modified_ms": 6 }]),
    );
    let file = call(&mut output);
    reply(
        &mut input,
        &file,
        true,
        json!({"bytes":STANDARD.encode(server_markdown.as_bytes())}),
    );
    let ark = call(&mut output);
    reply(&mut input, &ark, true, json!([]));
    let deletion = call(&mut output);
    assert_eq!(deletion["operation"], "filesystem.delete");
    assert_eq!(
        deletion["params"]["path"],
        format!("{}{}Renamed.md", vault_root, std::path::MAIN_SEPARATOR)
    );
    reply(&mut input, &deletion, true, Value::Null);
    let provenance = call(&mut output);
    assert_eq!(provenance["operation"], "ark.write");
    assert_eq!(provenance["params"]["params"]["state"], "missing");
    reply(&mut input, &provenance, true, Value::Null);
    let state_write = call(&mut output);
    reply(&mut input, &state_write, true, Value::Null);

    // Divergent ARK and Markdown edits create a bounded sibling artifact, never overwrite either side.
    writeln!(input, "{}", json!({"method":"worker.tick"})).unwrap();
    input.flush().unwrap();
    let state_read = call(&mut output);
    reply(
        &mut input,
        &state_read,
        true,
        json!({"bytes":STANDARD.encode(renamed_state.as_bytes())}),
    );
    let poll = call(&mut output);
    reply(
        &mut input,
        &poll,
        true,
        json!([{ "name":"Renamed.md", "kind":"file", "size": 80, "modified_ms": 3 }]),
    );
    let list = call(&mut output);
    reply(
        &mut input,
        &list,
        true,
        json!([{ "name":"Renamed.md", "kind":"file", "size": 80, "modified_ms": 3 }]),
    );
    let file = call(&mut output);
    reply(
        &mut input,
        &file,
        true,
        json!({"bytes":STANDARD.encode(markdown.replace("From ARK", "Local conflict").as_bytes())}),
    );
    let ark = call(&mut output);
    let remote = json!({"id":"note-1","type_id":"com.kosmos.note","type_version":"1.0.0","title":"One","props_json":{"description":null,"extensions":{}},"content_json":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Remote conflict"}]}]},"created_at":"x","updated_at":"z","deleted_at":null});
    reply(&mut input, &ark, true, json!([remote]));
    let conflict = call(&mut output);
    assert_eq!(conflict["operation"], "filesystem.write");
    assert_eq!(
        conflict["params"]["path"],
        format!(
            "{}{}Renamed.md.ark-conflict.md",
            vault_root,
            std::path::MAIN_SEPARATOR
        )
    );
    reply(&mut input, &conflict, true, Value::Null);
    let provenance = call(&mut output);
    assert_eq!(provenance["operation"], "ark.write");
    assert_eq!(provenance["params"]["operation"], "external_refs.upsert");
    assert_eq!(provenance["params"]["params"]["state"], "conflict");
    assert!(provenance["params"]["params"]["hash"].as_str().is_some());
    assert!(provenance["params"]["params"]["revision"]
        .as_str()
        .is_some());
    reply(&mut input, &provenance, true, Value::Null);
    let state_write = call(&mut output);
    let conflicted_state = String::from_utf8(
        STANDARD
            .decode(state_write["params"]["bytes"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    reply(&mut input, &state_write, true, Value::Null);

    // Accepting the ARK candidate in the main file clears the pending
    // conflict state; the next rescan must not recreate the sibling.
    writeln!(input, "{}", json!({"method":"worker.tick"})).unwrap();
    input.flush().unwrap();
    let state_read = call(&mut output);
    reply(
        &mut input,
        &state_read,
        true,
        json!({"bytes":STANDARD.encode(conflicted_state.as_bytes())}),
    );
    let poll = call(&mut output);
    reply(
        &mut input,
        &poll,
        true,
        json!([{ "name":"Renamed.md", "kind":"file", "size": 80, "modified_ms": 4 }]),
    );
    let list = call(&mut output);
    reply(
        &mut input,
        &list,
        true,
        json!([{ "name":"Renamed.md", "kind":"file", "size": 80, "modified_ms": 4 }]),
    );
    let file = call(&mut output);
    let remote_markdown = markdown.replace("From ARK", "Remote conflict");
    reply(
        &mut input,
        &file,
        true,
        json!({"bytes":STANDARD.encode(remote_markdown.as_bytes())}),
    );
    let ark = call(&mut output);
    reply(&mut input, &ark, true, json!([remote]));
    let provenance = call(&mut output);
    assert_eq!(provenance["operation"], "ark.write");
    assert_eq!(provenance["params"]["operation"], "external_refs.upsert");
    assert_eq!(provenance["params"]["params"]["state"], "clean");
    reply(&mut input, &provenance, true, Value::Null);
    let replacement = call(&mut output);
    assert_eq!(replacement["operation"], "filesystem.delete");
    assert_eq!(
        replacement["params"]["path"],
        format!(
            "{}{}Renamed.md.ark-conflict.md",
            vault_root,
            std::path::MAIN_SEPARATOR
        )
    );
    reply(&mut input, &replacement, true, Value::Null);
    let state_write = call(&mut output);
    let resolved_state = String::from_utf8(
        STANDARD
            .decode(state_write["params"]["bytes"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    let resolved_json: Value = serde_json::from_str(&resolved_state).unwrap();
    assert!(resolved_json["records"]["note-1"]["conflict"].is_null());
    reply(&mut input, &state_write, true, Value::Null);

    // A deliberate edit after conflict is imported once and also converges.
    writeln!(input, "{}", json!({"method":"worker.tick"})).unwrap();
    input.flush().unwrap();
    let state_read = call(&mut output);
    reply(
        &mut input,
        &state_read,
        true,
        json!({"bytes":STANDARD.encode(conflicted_state.as_bytes())}),
    );
    let poll = call(&mut output);
    reply(
        &mut input,
        &poll,
        true,
        json!([{ "name":"Renamed.md", "kind":"file", "size": 80, "modified_ms": 5 }]),
    );
    let list = call(&mut output);
    reply(
        &mut input,
        &list,
        true,
        json!([{ "name":"Renamed.md", "kind":"file", "size": 80, "modified_ms": 5 }]),
    );
    let file = call(&mut output);
    let deliberate = remote_markdown.replace("Remote conflict", "Deliberate");
    reply(
        &mut input,
        &file,
        true,
        json!({"bytes":STANDARD.encode(deliberate.as_bytes())}),
    );
    let ark = call(&mut output);
    reply(&mut input, &ark, true, json!([remote]));
    let upsert = call(&mut output);
    assert_eq!(upsert["operation"], "ark.write");
    assert_eq!(upsert["params"]["operation"], "upsert_object");
    reply(&mut input, &upsert, true, Value::Null);
    let get = call(&mut output);
    reply(
        &mut input,
        &get,
        true,
        json!({"id":"note-1","type_id":"com.kosmos.note","type_version":"1.0.0","title":"One","props_json":{"description":null,"extensions":{}},"content_json":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Deliberate"}]}]}}),
    );
    let provenance = call(&mut output);
    assert_eq!(provenance["operation"], "ark.write");
    assert_eq!(provenance["params"]["operation"], "external_refs.upsert");
    assert_eq!(provenance["params"]["params"]["state"], "clean");
    reply(&mut input, &provenance, true, Value::Null);
    let replacement = call(&mut output);
    assert_eq!(replacement["operation"], "filesystem.delete");
    reply(&mut input, &replacement, true, Value::Null);
    let state_write = call(&mut output);
    let converged: Value = serde_json::from_slice(
        &STANDARD
            .decode(state_write["params"]["bytes"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert!(converged["records"]["note-1"]["conflict"].is_null());
    reply(&mut input, &state_write, true, Value::Null);

    writeln!(
        input,
        "{}",
        json!({"method":"worker.stop","generation":1,"reason":"test"})
    )
    .unwrap();
    input.flush().unwrap();
    assert!(child
        .wait_timeout(Duration::from_secs(5))
        .unwrap()
        .is_some());
}

trait WaitTimeout {
    fn wait_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>>;
}
impl WaitTimeout for std::process::Child {
    fn wait_timeout(
        &mut self,
        timeout: Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>> {
        let start = std::time::Instant::now();
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(Some(status));
            }
            if start.elapsed() >= timeout {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
