#![allow(clippy::unwrap_used)]

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use sha2::Digest as _;
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

#[test]
fn divergent_untracked_file_conflicts_instead_of_overwriting() {
    // A vault file carrying a known ark_id with no recorded baseline may hold
    // user content the bridge has never seen (fresh state, restored vault,
    // second device). It must become a conflict, never a silent overwrite.
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
    let divergent = "---\nark_id: \"note-1\"\nark_type: \"com.kosmos.note\"\nark_version: \"1.0.0\"\nbridge_version: 1\ntitle: \"One\"\n---\n\nLocal draft\n";

    let state_read = call(&mut output);
    reply(&mut input, &state_read, false, Value::Null);
    let poll = call(&mut output);
    reply(
        &mut input,
        &poll,
        true,
        json!([{ "name":"Draft-note-1.md", "kind":"file", "size": 80, "modified_ms": 1 }]),
    );
    let list = call(&mut output);
    reply(
        &mut input,
        &list,
        true,
        json!([{ "name":"Draft-note-1.md", "kind":"file", "size": 80, "modified_ms": 1 }]),
    );
    let file = call(&mut output);
    assert_eq!(file["operation"], "filesystem.read");
    reply(
        &mut input,
        &file,
        true,
        json!({"bytes":STANDARD.encode(divergent.as_bytes())}),
    );
    let ark = call(&mut output);
    assert_eq!(ark["operation"], "ark.read");
    reply(&mut input, &ark, true, json!([object]));

    let mut saw_conflict_write = false;
    for _ in 0..3 {
        let message = call(&mut output);
        match message["operation"].as_str().unwrap() {
            "filesystem.read" => reply(
                &mut input,
                &message,
                true,
                json!({"bytes":STANDARD.encode(divergent.as_bytes())}),
            ),
            "filesystem.write" => {
                let path = message["params"]["path"].as_str().unwrap();
                if path.ends_with(".md") {
                    assert!(
                        path.ends_with(".ark-conflict.md"),
                        "ARK candidate must land in the conflict sibling, got {path}"
                    );
                    saw_conflict_write = true;
                }
                reply(&mut input, &message, true, Value::Null);
            }
            "ark.write" => {
                if message["params"]["operation"] == "external_refs.upsert" {
                    assert_eq!(message["params"]["params"]["state"], "conflict");
                }
                reply(&mut input, &message, true, Value::Null);
            }
            other => panic!("unexpected broker call {other}"),
        }
    }
    assert!(saw_conflict_write, "no conflict sibling write observed");

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

#[test]
fn provenance_revision_reads_camelcase_updated_at() {
    // Objects upserted by source workers carry camelCase fields
    // (typeId/propsJson/updatedAt). Provenance must attribute the same
    // revision the object carries instead of silently dropping it.
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

    let object = json!({"id":"note-1","typeId":"com.kosmos.note","typeVersion":"1.0.0","title":"One","propsJson":{"body":"From ARK"},"createdAt":"x","updatedAt":"2026-09-01T00:00:00Z","deletedAt":null});
    let state_read = call(&mut output);
    reply(&mut input, &state_read, false, Value::Null);
    let poll = call(&mut output);
    reply(&mut input, &poll, true, json!([]));
    let list = call(&mut output);
    reply(&mut input, &list, true, json!([]));
    let ark = call(&mut output);
    reply(&mut input, &ark, true, json!([object]));
    let compare = call(&mut output);
    assert_eq!(compare["operation"], "filesystem.read");
    reply(&mut input, &compare, false, Value::Null);
    let projection = call(&mut output);
    assert_eq!(projection["operation"], "filesystem.write");
    reply(&mut input, &projection, true, Value::Null);
    let provenance = call(&mut output);
    assert_eq!(provenance["params"]["operation"], "external_refs.upsert");
    assert_eq!(
        provenance["params"]["params"]["revision"], "2026-09-01T00:00:00Z",
        "camelCase updatedAt must reach external_refs.revision"
    );
    reply(&mut input, &provenance, true, Value::Null);

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

#[test]
fn missing_object_deletes_the_verified_file_not_a_stale_path() {
    // The recorded projection path can go stale while the bridge is offline
    // (vault restored, file renamed and the ARK object removed in the same
    // window). Deletion must target the file whose content was actually
    // verified, not the recorded path.
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

    let content = "---\nark_id: \"note-1\"\nark_type: \"com.kosmos.note\"\nark_version: \"1.0.0\"\nbridge_version: 1\ntitle: \"One\"\n---\n\nBody\n";
    let file_hash = format!("{:x}", sha2::Sha256::digest(content.as_bytes()));
    let state = json!({"records":{"note-1":{"path":"Old-note-1.md","file_hash":file_hash,"ark_hash":"ark","conflict":null}},"last_sync":null,"conflict_count":0,"last_conflict_at":null});

    let state_read = call(&mut output);
    assert_eq!(state_read["operation"], "filesystem.read");
    reply(
        &mut input,
        &state_read,
        true,
        json!({"bytes":STANDARD.encode(state.to_string().as_bytes())}),
    );
    let poll = call(&mut output);
    reply(
        &mut input,
        &poll,
        true,
        json!([{ "name":"Renamed.md", "kind":"file", "size": 80, "modified_ms": 1 }]),
    );
    let list = call(&mut output);
    reply(
        &mut input,
        &list,
        true,
        json!([{ "name":"Renamed.md", "kind":"file", "size": 80, "modified_ms": 1 }]),
    );
    let file = call(&mut output);
    assert_eq!(file["operation"], "filesystem.read");
    reply(
        &mut input,
        &file,
        true,
        json!({"bytes":STANDARD.encode(content.as_bytes())}),
    );
    let ark = call(&mut output);
    assert_eq!(ark["operation"], "ark.read");
    reply(&mut input, &ark, true, json!([]));

    let deletion = call(&mut output);
    assert_eq!(deletion["operation"], "filesystem.delete");
    assert_eq!(
        deletion["params"]["path"],
        format!("{}{}Renamed.md", vault_root, std::path::MAIN_SEPARATOR),
        "must delete the file whose content was verified, not the stale recorded path"
    );
    reply(&mut input, &deletion, true, Value::Null);
    let provenance = call(&mut output);
    assert_eq!(provenance["operation"], "ark.write");
    assert_eq!(provenance["params"]["params"]["state"], "missing");
    reply(&mut input, &provenance, true, Value::Null);
    let state_write = call(&mut output);
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

#[test]
fn tombstoned_object_deletes_the_verified_file() {
    // list_objects returns soft-deleted rows: an object whose deleted_at is
    // set is gone for sync purposes. Its verified projection must be removed
    // and provenance recorded as missing — the same as an absent object —
    // instead of being re-rendered as live content.
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

    let content = "---\nark_id: \"note-1\"\nark_type: \"com.kosmos.note\"\nark_version: \"1.0.0\"\nbridge_version: 1\ntitle: \"One\"\n---\n\nBody\n";
    let file_hash = format!("{:x}", sha2::Sha256::digest(content.as_bytes()));
    let state = json!({"records":{"note-1":{"path":"One-note-1.md","file_hash":file_hash,"ark_hash":"ark","conflict":null}},"last_sync":null,"conflict_count":0,"last_conflict_at":null});

    let state_read = call(&mut output);
    assert_eq!(state_read["operation"], "filesystem.read");
    reply(
        &mut input,
        &state_read,
        true,
        json!({"bytes":STANDARD.encode(state.to_string().as_bytes())}),
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
    reply(
        &mut input,
        &file,
        true,
        json!({"bytes":STANDARD.encode(content.as_bytes())}),
    );
    let ark = call(&mut output);
    assert_eq!(ark["operation"], "ark.read");
    let tombstone = json!({"id":"note-1","type_id":"com.kosmos.note","type_version":"1.0.0","title":"One","props_json":{},"content_json":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Deleted"}]}]},"created_at":"x","updated_at":"x","deleted_at":"2026-09-20T00:00:00Z"});
    reply(&mut input, &ark, true, json!([tombstone]));

    let deletion = call(&mut output);
    assert_eq!(
        deletion["operation"], "filesystem.delete",
        "a soft-deleted object must remove its verified projection"
    );
    assert_eq!(
        deletion["params"]["path"],
        format!("{}{}One-note-1.md", vault_root, std::path::MAIN_SEPARATOR)
    );
    reply(&mut input, &deletion, true, Value::Null);
    let provenance = call(&mut output);
    assert_eq!(provenance["operation"], "ark.write");
    assert_eq!(provenance["params"]["params"]["state"], "missing");
    reply(&mut input, &provenance, true, Value::Null);
    let state_write = call(&mut output);
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

#[test]
fn tombstoned_object_does_not_resurrect_a_file() {
    // With no baseline record and no vault file, a soft-deleted object must
    // not produce a Markdown projection: the sync goes straight to durable
    // state instead of writing a file for deleted content.
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

    let state_read = call(&mut output);
    assert_eq!(state_read["operation"], "filesystem.read");
    reply(&mut input, &state_read, false, Value::Null);
    let poll = call(&mut output);
    assert_eq!(poll["operation"], "filesystem.poll");
    reply(&mut input, &poll, true, json!([]));
    let list = call(&mut output);
    reply(&mut input, &list, true, json!([]));
    let ark = call(&mut output);
    assert_eq!(ark["operation"], "ark.read");
    let tombstone = json!({"id":"note-1","type_id":"com.kosmos.note","type_version":"1.0.0","title":"One","props_json":{},"content_json":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Deleted"}]}]},"created_at":"x","updated_at":"x","deleted_at":"2026-09-20T00:00:00Z"});
    reply(&mut input, &ark, true, json!([tombstone]));

    let after = call(&mut output);
    assert_eq!(
        after["operation"], "filesystem.write",
        "expected durable state write, got {}",
        after["operation"]
    );
    assert_eq!(
        after["params"]["path"],
        format!("{}{}state.json", state_root, std::path::MAIN_SEPARATOR),
        "a tombstoned object must not write a vault file"
    );
    reply(&mut input, &after, true, Value::Null);

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

#[test]
fn heartbeat_continues_while_a_broker_call_is_pending() {
    // The supervisor reaps a worker after ~60s without worker.heartbeat. A
    // sync blocked inside a broker call must still heartbeat: leave the first
    // call unanswered and require worker.heartbeat within ~30s of silence.
    let temp = tempfile::tempdir().unwrap();
    let vault = temp.path().join("vault");
    let state_root = temp.path().join("state");
    std::fs::create_dir(&vault).unwrap();
    std::fs::create_dir(&state_root).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ark-markdown-bridge"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(input, "{}", json!({"method":"worker.bootstrap","package_id":"ark-markdown-bridge","version":"1.0.0","hash":"a","pid":1,"api_version":1,"generation":1,"correlation_id":"test","token":"token","bridge_config":{"vault_root":vault.to_string_lossy().into_owned(),"state_root":state_root.to_string_lossy().into_owned(),"selected_types":["com.kosmos.note"],"editable_fields":["title","body"],"readonly_fields":[]}})).unwrap();
    input.flush().unwrap();

    // Read worker stdout on a channel so the test can wait for a heartbeat
    // without blocking forever when none comes.
    let (sender, receiver) = std::sync::mpsc::channel::<Value>();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        let mut output = BufReader::new(stdout);
        let mut line = String::new();
        while output.read_line(&mut line).unwrap_or(0) > 0 {
            if let Ok(value) = serde_json::from_str(&line) {
                if sender.send(value).is_err() {
                    break;
                }
            }
            line.clear();
        }
    });

    let next_line = |wait: Duration| -> Option<Value> { receiver.recv_timeout(wait).ok() };
    assert_eq!(
        next_line(Duration::from_secs(5)).unwrap()["method"],
        "worker.hello"
    );
    let pending = next_line(Duration::from_secs(5)).unwrap();
    assert_eq!(pending["method"], "worker.call");
    // Deliberately unanswered: liveness must not depend on sync progress.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let saw_heartbeat = loop {
        match next_line(Duration::from_secs(35)) {
            Some(value) if value["method"] == "worker.heartbeat" => break true,
            Some(_) => {
                if std::time::Instant::now() > deadline {
                    break false;
                }
            }
            None => break false,
        }
    };
    assert!(
        saw_heartbeat,
        "no worker.heartbeat within 30s while a broker call was pending"
    );

    drop(input);
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
