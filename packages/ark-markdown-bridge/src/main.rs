//! First-party Package v1 bridge worker. All ARK and filesystem access goes
//! through the Engine broker on stdio; this binary never opens SQLite or files.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use kosmos_package_protocol::BridgeStatus;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{BufRead, Write},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

const MAX_FILES: usize = 4096;
const TICK: Duration = Duration::from_secs(2);
const BRIDGE_FORMAT_VERSION: u64 = 1;
const CANONICAL_VERSION: &str = "1.0.0";

fn canonical_type(kind: &str) -> Option<&'static str> {
    match kind {
        "com.kosmos.note" | "note_obj" => Some("com.kosmos.note"),
        "com.kosmos.task" | "task_obj" => Some("com.kosmos.task"),
        _ => None,
    }
}

#[derive(Clone, Deserialize)]
struct Config {
    vault_root: String,
    state_root: String,
    selected_types: Vec<String>,
    editable_fields: Vec<String>,
    readonly_fields: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    records: BTreeMap<String, RecordState>,
    #[serde(default)]
    last_sync: Option<String>,
    #[serde(default)]
    conflict_count: u32,
    #[serde(default)]
    last_conflict_at: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
struct RecordState {
    path: String,
    file_hash: String,
    ark_hash: String,
    #[serde(default)]
    conflict: Option<ConflictState>,
}

#[derive(Clone, Serialize, Deserialize)]
struct ConflictState {
    file_hash: String,
    ark_hash: String,
}

struct Client {
    token: String,
    generation: u64,
    next_id: u64,
    inbound: Receiver<Value>,
    stopped: bool,
}

impl Client {
    fn send(value: Value) -> Result<(), ()> {
        let mut out = std::io::stdout().lock();
        serde_json::to_writer(&mut out, &value).map_err(|_| ())?;
        out.write_all(b"\n").map_err(|_| ())?;
        out.flush().map_err(|_| ())
    }

    fn heartbeat(&self, status: Option<&BridgeStatus>) {
        let _ = Self::send(
            json!({"method":"worker.heartbeat","generation":self.generation,"token":self.token,"bridge_status":status}),
        );
    }

    fn call(&mut self, operation: &str, params: Value) -> Result<Value, ()> {
        let id = format!("bridge-{}", self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        Self::send(
            json!({"method":"worker.call","id":id,"generation":self.generation,"token":self.token,"operation":operation,"params":params}),
        )?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let message = self.inbound.recv_timeout(left).map_err(|_| ())?;
            if message.get("method").and_then(Value::as_str) == Some("worker.stop") {
                self.stopped = true;
                return Err(());
            }
            if message.get("method").and_then(Value::as_str) == Some("worker.result")
                && message.get("id").and_then(Value::as_str) == Some(&id)
            {
                return message
                    .get("ok")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    .then(|| message.get("result").cloned().unwrap_or(Value::Null))
                    .ok_or(());
            }
        }
    }

    fn ark(&mut self, write: bool, operation: &str, params: Value) -> Result<Value, ()> {
        self.call(
            if write { "ark.write" } else { "ark.read" },
            json!({"operation":operation,"params":params}),
        )
    }
    fn fs(&mut self, operation: &str, path: &str, bytes: Option<&[u8]>) -> Result<Value, ()> {
        let mut params = Map::new();
        params.insert("path".into(), Value::String(path.into()));
        if let Some(bytes) = bytes {
            params.insert("bytes".into(), Value::String(STANDARD.encode(bytes)));
        }
        self.call(operation, Value::Object(params))
    }
    fn read(&mut self, path: &str) -> Result<String, ()> {
        let value = self.fs("filesystem.read", path, None)?;
        let bytes = value
            .get("bytes")
            .and_then(Value::as_str)
            .and_then(|value| STANDARD.decode(value).ok())
            .ok_or(())?;
        String::from_utf8(bytes).map_err(|_| ())
    }
    fn write(&mut self, path: &str, value: &str) -> Result<(), ()> {
        self.fs("filesystem.write", path, Some(value.as_bytes()))
            .map(|_| ())
    }
}

fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn join(root: &str, leaf: &str) -> String {
    format!(
        "{}{}{}",
        root.trim_end_matches(['/', '\\']),
        std::path::MAIN_SEPARATOR,
        leaf
    )
}
fn scalar(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}
fn safe_name(title: &str, id: &str) -> String {
    let stem = safe_stem(title);
    format!(
        "{}-{}.md",
        if stem.is_empty() { "untitled" } else { &stem },
        &id[..id.len().min(12)]
    )
}
fn safe_stem(title: &str) -> String {
    let stem: String = title
        .chars()
        .map(|c| {
            if "<>:\"/\\|?".contains(c) || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    stem.trim().chars().take(96).collect::<String>()
}
fn collision_safe_name(title: &str, id: &str) -> String {
    let stem = safe_stem(title);
    let suffix = &hash(id)[..16];
    format!(
        "{}-{}-{}.md",
        if stem.is_empty() { "untitled" } else { &stem },
        &id[..id.len().min(12)],
        suffix
    )
}

fn collision_key(title: &str, id: &str) -> String {
    // NTFS is case-insensitive; Unicode lowercasing keeps the decision stable
    // for non-ASCII titles too.
    format!(
        "{}-{}",
        safe_stem(title).to_lowercase(),
        id[..id.len().min(12)].to_lowercase()
    )
}

fn object_type(object: &Value) -> Option<&str> {
    object
        .get("type_id")
        .or_else(|| object.get("typeId"))
        .and_then(Value::as_str)
}
fn object_body(object: &Value) -> String {
    if let Some(content) = object
        .get("content_json")
        .or_else(|| object.get("contentJson"))
    {
        let mut out = String::new();
        rich_text_to_markdown(content, &mut out);
        if !out.is_empty() {
            return out.trim_end().to_owned();
        }
    }
    object
        .get("props_json")
        .or_else(|| object.get("propsJson"))
        .and_then(|value| value.get("body"))
        .and_then(Value::as_str)
        .or_else(|| {
            object
                .get("content_json")
                .or_else(|| object.get("contentJson"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default()
        .into()
}

fn rich_text_to_markdown(value: &Value, out: &mut String) {
    match value.get("type").and_then(Value::as_str).unwrap_or("") {
        "doc" => children_to_markdown(value, out),
        "paragraph" => {
            inline_to_markdown(value, out);
            out.push_str("\n\n");
        }
        "heading" => {
            let level = value
                .get("attrs")
                .and_then(|a| a.get("level"))
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 6);
            out.push_str(&"#".repeat(level as usize));
            out.push(' ');
            inline_to_markdown(value, out);
            out.push_str("\n\n");
        }
        "text" => out.push_str(value.get("text").and_then(Value::as_str).unwrap_or("")),
        "hardBreak" | "hard_break" => out.push_str("  \n"),
        "blockquote" => {
            let mut inner = String::new();
            children_to_markdown(value, &mut inner);
            for line in inner.trim_end().lines() {
                out.push_str("> ");
                out.push_str(line);
                out.push('\n');
            }
            out.push('\n');
        }
        "bulletList" | "bullet_list" => list_to_markdown(value, out, false),
        "orderedList" | "ordered_list" => list_to_markdown(value, out, true),
        "codeBlock" | "code_block" => {
            out.push_str("```");
            if let Some(lang) = value
                .get("attrs")
                .and_then(|a| a.get("language"))
                .and_then(Value::as_str)
            {
                out.push_str(lang);
            }
            out.push('\n');
            out.push_str(&collect_text(value));
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("```\n\n");
        }
        "horizontalRule" | "horizontal_rule" => out.push_str("---\n\n"),
        "listItem" | "list_item" => children_to_markdown(value, out),
        _ => {}
    }
}

fn children_to_markdown(value: &Value, out: &mut String) {
    if let Some(items) = value.get("content").and_then(Value::as_array) {
        for item in items {
            rich_text_to_markdown(item, out);
        }
    }
}

fn inline_to_markdown(value: &Value, out: &mut String) {
    if let Some(items) = value.get("content").and_then(Value::as_array) {
        for item in items {
            rich_text_to_markdown(item, out);
        }
    }
}

fn list_to_markdown(value: &Value, out: &mut String, ordered: bool) {
    if let Some(items) = value.get("content").and_then(Value::as_array) {
        for (index, item) in items.iter().enumerate() {
            if ordered {
                out.push_str(&format!("{}. ", index + 1));
            } else {
                out.push_str("- ");
            }
            let mut inner = String::new();
            children_to_markdown(item, &mut inner);
            out.push_str(inner.trim());
            out.push('\n');
        }
        out.push('\n');
    }
}

fn collect_text(value: &Value) -> String {
    let mut out = String::new();
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        out.push_str(text);
    }
    if let Some(items) = value.get("content").and_then(Value::as_array) {
        for item in items {
            out.push_str(&collect_text(item));
        }
    }
    out
}
fn render(object: &Value, config: &Config) -> Option<String> {
    let _ = &config.readonly_fields; // intentionally never imported from Markdown
    let id = object.get("id")?.as_str()?;
    let kind = object_type(object)?;
    let kind = canonical_type(kind).unwrap_or(kind);
    let title = object
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut lines = vec![
        "---".into(),
        format!("ark_id: {}", scalar(id)),
        format!("ark_type: {}", scalar(kind)),
        format!(
            "ark_version: {}",
            scalar(
                object
                    .get("type_version")
                    .or_else(|| object.get("typeVersion"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
            )
        ),
        format!("bridge_version: {}", BRIDGE_FORMAT_VERSION),
        format!("title: {}", scalar(title)),
    ];
    let props = object
        .get("props_json")
        .or_else(|| object.get("propsJson"))
        .and_then(Value::as_object);
    for field in &config.editable_fields {
        if field != "title" && field != "body" {
            if let Some(value) = props.and_then(|props| props.get(field)) {
                lines.push(format!("{field}: {value}"));
            }
        }
    }
    lines.push("---".into());
    lines.push(String::new());
    lines.push(object_body(object));
    Some(lines.join("\n").trim_end().to_owned() + "\n")
}

fn parse(
    markdown: &str,
    config: &Config,
) -> Option<(String, String, HashMap<String, Value>, String)> {
    let markdown = markdown
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n");
    let rest = markdown.strip_prefix("---\n")?;
    let (front, body) = rest.split_once("\n---")?;
    let mut fields = HashMap::new();
    for line in front.lines() {
        let (key, raw) = line.split_once(':')?;
        let key = key.trim();
        let raw = raw.trim();
        fields.insert(
            key.into(),
            serde_json::from_str(raw)
                .unwrap_or_else(|_| Value::String(raw.trim_matches(['\'', '\"']).into())),
        );
    }
    let id = fields.remove("ark_id")?.as_str()?.to_owned();
    let source_kind = fields.remove("ark_type")?.as_str()?.to_owned();
    let kind = canonical_type(&source_kind)
        .unwrap_or(&source_kind)
        .to_owned();
    let version = fields
        .remove("ark_version")
        .and_then(|value| value.as_str().map(str::to_owned));
    if fields.remove("bridge_version")?.as_u64()? != BRIDGE_FORMAT_VERSION {
        return None;
    }
    if canonical_type(&source_kind).is_some() && version.as_deref() != Some(CANONICAL_VERSION) {
        return None;
    }
    if !config
        .selected_types
        .iter()
        .any(|item| item == &source_kind || item == &kind)
    {
        return None;
    }
    fields.retain(|key, _| config.editable_fields.iter().any(|field| field == key));
    Some((
        id,
        kind,
        fields,
        body.trim_start_matches('\n')
            .trim_end_matches('\n')
            .to_owned(),
    ))
}

fn list_files(client: &mut Client, config: &Config) -> Result<Vec<String>, ()> {
    // Poll is the cheap change signal; list remains the authoritative bounded directory snapshot.
    client.fs("filesystem.poll", &config.vault_root, None)?;
    let entries = client.fs("filesystem.list", &config.vault_root, None)?;
    Ok(entries
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            (entry.get("kind").and_then(Value::as_str) == Some("file"))
                .then(|| entry.get("name").and_then(Value::as_str))
                .flatten()
                .filter(|name| name.ends_with(".md") && !name.ends_with(".ark-conflict.md"))
                .map(str::to_owned)
        })
        .take(MAX_FILES)
        .collect())
}

fn load_state(client: &mut Client, config: &Config) -> State {
    client
        .read(&join(&config.state_root, "state.json"))
        .ok()
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default()
}
fn save_state(client: &mut Client, config: &Config, state: &State) -> Result<(), ()> {
    let value = serde_json::to_string(state).map_err(|_| ())?;
    client.write(&join(&config.state_root, "state.json"), &value)
}

fn record_provenance(
    client: &mut Client,
    object_id: &str,
    state: &str,
    revision: Option<&str>,
    content_hash: Option<&str>,
) -> Result<(), ()> {
    let now = chrono::Utc::now().to_rfc3339();
    client
        .ark(
            true,
            "external_refs.upsert",
            json!({
                "connectorId": "markdown",
                "accountId": "bridge",
                "externalType": "markdown",
                "externalId": object_id,
                "objectId": object_id,
                "hash": content_hash,
                "revision": revision,
                "state": state,
                "lastPulledAt": now,
                "lastPushedAt": now
            }),
        )
        .map(|_| ())
}

fn apply_editable(
    mut object: Value,
    fields: HashMap<String, Value>,
    body: String,
    config: &Config,
) -> Option<Value> {
    let mut changed = false;
    if config.editable_fields.iter().any(|field| field == "title") {
        if let Some(title) = fields.get("title").and_then(Value::as_str) {
            if object.get("title").and_then(Value::as_str) != Some(title) {
                object["title"] = Value::String(title.into());
                changed = true;
            }
        }
    }
    let is_canonical = canonical_type(object_type(&object).unwrap_or_default()).is_some();
    if is_canonical {
        let version = object
            .get("type_version")
            .or_else(|| object.get("typeVersion"))
            .and_then(Value::as_str);
        if version != Some(CANONICAL_VERSION) {
            return None;
        }
        if config.editable_fields.iter().any(|field| field == "body") {
            let paragraphs = body
                .split("\n\n")
                .map(|paragraph| {
                    json!({
                        "type": "paragraph",
                        "content": [{"type": "text", "text": paragraph}]
                    })
                })
                .collect::<Vec<_>>();
            let content = json!({"type": "doc", "content": paragraphs});
            let key = if object.get("content_json").is_some() {
                "content_json"
            } else {
                "contentJson"
            };
            if object.get(key) != Some(&content) {
                object[key] = content;
                changed = true;
            }
        }
    }
    let props = if object.get("props_json").is_some() {
        object.get_mut("props_json")
    } else {
        object.get_mut("propsJson")
    }
    .and_then(Value::as_object_mut);
    let Some(props) = props else {
        return changed.then_some(object);
    };
    for (field, value) in fields {
        if field == "title"
            || field == "body"
            || !config.editable_fields.iter().any(|item| item == &field)
        {
            continue;
        }
        if props.get(&field) != Some(&value) {
            props.insert(field, value);
            changed = true;
        }
    }
    if !is_canonical
        && config.editable_fields.iter().any(|field| field == "body")
        && props.get("body").and_then(Value::as_str) != Some(body.as_str())
    {
        props.insert("body".into(), Value::String(body));
        changed = true;
    }
    changed.then_some(object)
}

fn sync(client: &mut Client, config: &Config) -> Result<BridgeStatus, ()> {
    let mut state = load_state(client, config);
    let files = list_files(client, config)?;
    let mut by_id = HashMap::new();
    for path in &files {
        if let Ok(content) = client.read(&join(&config.vault_root, path)) {
            if let Some((id, _, _, _)) = parse(&content, config) {
                by_id.insert(id, (path.clone(), content));
            }
        }
    }
    let objects = client
        .ark(false, "list_objects", Value::Null)?
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut collision_ids: HashMap<String, HashSet<String>> = HashMap::new();
    for object in &objects {
        if let (Some(id), Some(kind)) = (
            object.get("id").and_then(Value::as_str),
            object_type(object),
        ) {
            if config.selected_types.iter().any(|item| {
                item == kind || canonical_type(kind).is_some_and(|canonical| item == canonical)
            }) {
                let title = object
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                collision_ids
                    .entry(collision_key(title, id))
                    .or_default()
                    .insert(id.into());
            }
        }
    }
    let mut seen = HashMap::new();
    for object in objects.into_iter().filter(|object| {
        object_type(object).is_some_and(|kind| {
            config.selected_types.iter().any(|item| {
                item == kind || canonical_type(kind).is_some_and(|canonical| item == canonical)
            })
        })
    }) {
        let Some(id) = object.get("id").and_then(Value::as_str).map(str::to_owned) else {
            continue;
        };
        let Some(ark_content) = render(&object, config) else {
            continue;
        };
        let ark_hash = hash(&ark_content);
        let mut previous = state.records.get(&id).cloned();
        let had_conflict = previous
            .as_ref()
            .and_then(|item| item.conflict.as_ref())
            .is_some();
        let (path, file_content) = by_id.remove(&id).unwrap_or_else(|| {
            (
                previous
                    .as_ref()
                    .map(|item| item.path.clone())
                    .unwrap_or_else(|| {
                        let title = object
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let key = collision_key(title, &id);
                        if collision_ids.get(&key).is_some_and(|ids| ids.len() > 1) {
                            collision_safe_name(title, &id)
                        } else {
                            safe_name(title, &id)
                        }
                    }),
                String::new(),
            )
        });
        let mut file_changed = previous
            .as_ref()
            .is_some_and(|item| !file_content.is_empty() && hash(&file_content) != item.file_hash);
        let mut ark_changed = previous
            .as_ref()
            .is_some_and(|item| item.ark_hash != ark_hash);
        seen.insert(id.clone(), true);

        if let Some(pending) = previous.as_ref().and_then(|item| item.conflict.clone()) {
            let current_file_hash = (!file_content.is_empty()).then(|| hash(&file_content));
            match (
                current_file_hash.as_deref(),
                pending.file_hash.as_str(),
                pending.ark_hash == ark_hash,
            ) {
                (Some(file_hash), pending_file, true) if file_hash == pending_file => {
                    // Neither side changed since the last conflict; keep the
                    // user's file and the ARK candidate without re-emitting it.
                    continue;
                }
                (Some(file_hash), _, true) if file_hash == ark_hash => {
                    // User accepted the ARK candidate in the main file.
                    record_provenance(
                        client,
                        &id,
                        "clean",
                        object.get("updated_at").and_then(Value::as_str),
                        Some(&ark_hash),
                    )?;
                    client.fs(
                        "filesystem.delete",
                        &join(&config.vault_root, &format!("{path}.ark-conflict.md")),
                        None,
                    )?;
                    state.records.insert(
                        id.clone(),
                        RecordState {
                            path,
                            file_hash: ark_hash.clone(),
                            ark_hash,
                            conflict: None,
                        },
                    );
                    continue;
                }
                (Some(file_hash), pending_file, false) if file_hash == pending_file => {
                    // ARK changed again while the main file stayed unresolved.
                    client.write(
                        &join(&config.vault_root, &format!("{path}.ark-conflict.md")),
                        &ark_content,
                    )?;
                    record_provenance(
                        client,
                        &id,
                        "conflict",
                        object.get("updated_at").and_then(Value::as_str),
                        Some(&ark_hash),
                    )?;
                    state.records.insert(
                        id.clone(),
                        RecordState {
                            path,
                            file_hash: file_hash.into(),
                            ark_hash: ark_hash.clone(),
                            conflict: Some(ConflictState {
                                file_hash: file_hash.into(),
                                ark_hash,
                            }),
                        },
                    );
                    continue;
                }
                (Some(file_hash), _, false) => {
                    // Both sides changed again: keep the main file untouched
                    // and refresh the deterministic ARK candidate sibling.
                    client.write(
                        &join(&config.vault_root, &format!("{path}.ark-conflict.md")),
                        &ark_content,
                    )?;
                    record_provenance(
                        client,
                        &id,
                        "conflict",
                        object.get("updated_at").and_then(Value::as_str),
                        Some(&ark_hash),
                    )?;
                    state.records.insert(
                        id.clone(),
                        RecordState {
                            path,
                            file_hash: file_hash.into(),
                            ark_hash: ark_hash.clone(),
                            conflict: Some(ConflictState {
                                file_hash: file_hash.into(),
                                ark_hash,
                            }),
                        },
                    );
                    continue;
                }
                (Some(_), _, true) => {
                    // A deliberate main-file edit is resolved through the
                    // normal editable-field import path below.
                    previous = Some(RecordState {
                        path: previous
                            .as_ref()
                            .map(|item| item.path.clone())
                            .unwrap_or(path.clone()),
                        file_hash: pending.file_hash,
                        ark_hash: pending.ark_hash,
                        conflict: None,
                    });
                    file_changed = true;
                    ark_changed = false;
                }
                _ => continue,
            }
        }
        if file_changed && ark_changed {
            client.write(
                &join(&config.vault_root, &format!("{path}.ark-conflict.md")),
                &ark_content,
            )?;
            record_provenance(
                client,
                &id,
                "conflict",
                object.get("updated_at").and_then(Value::as_str),
                Some(&ark_hash),
            )?;
            state.conflict_count = state.conflict_count.saturating_add(1);
            state.last_conflict_at = Some(chrono::Utc::now().to_rfc3339());
            state.records.insert(
                id.clone(),
                RecordState {
                    path,
                    file_hash: hash(&file_content),
                    ark_hash: ark_hash.clone(),
                    conflict: Some(ConflictState {
                        file_hash: hash(&file_content),
                        ark_hash,
                    }),
                },
            );
            continue;
        }
        if file_changed {
            let Some((parsed_id, parsed_type, fields, body)) = parse(&file_content, config) else {
                continue;
            };
            if parsed_id != id
                || canonical_type(&parsed_type).unwrap_or(parsed_type.as_str())
                    != canonical_type(object_type(&object).unwrap_or_default())
                        .unwrap_or(object_type(&object).unwrap_or_default())
            {
                continue;
            }
            let Some(mut updated) = apply_editable(object.clone(), fields, body, config) else {
                continue;
            };
            if let Some(canonical) = canonical_type(&parsed_type) {
                updated["type_id"] = Value::String(canonical.into());
                updated["type_version"] = Value::String(CANONICAL_VERSION.into());
            }
            client.ark(true, "upsert_object", json!({"object":updated}))?;
            let refreshed = client.ark(false, "get_object", json!({"id":id}))?;
            let rendered = render(&refreshed, config).ok_or(())?;
            if rendered != file_content {
                client.write(&join(&config.vault_root, &path), &rendered)?;
            }
            if had_conflict {
                record_provenance(
                    client,
                    &id,
                    "clean",
                    refreshed.get("updated_at").and_then(Value::as_str),
                    Some(&hash(&rendered)),
                )?;
                client.fs(
                    "filesystem.delete",
                    &join(&config.vault_root, &format!("{path}.ark-conflict.md")),
                    None,
                )?;
            } else {
                record_provenance(
                    client,
                    &id,
                    "clean",
                    refreshed.get("updated_at").and_then(Value::as_str),
                    Some(&hash(&rendered)),
                )?;
            }
            state.records.insert(
                id.clone(),
                RecordState {
                    path,
                    file_hash: hash(&rendered),
                    ark_hash: hash(&rendered),
                    conflict: None,
                },
            );
        } else {
            if file_content != ark_content {
                let full_path = join(&config.vault_root, &path);
                if client.read(&full_path).unwrap_or_default() != file_content {
                    continue;
                }
                client.write(&full_path, &ark_content)?;
            }
            record_provenance(
                client,
                &id,
                "clean",
                object.get("updated_at").and_then(Value::as_str),
                Some(&ark_hash),
            )?;
            state.records.insert(
                id.clone(),
                RecordState {
                    path,
                    file_hash: ark_hash.clone(),
                    ark_hash,
                    conflict: None,
                },
            );
        }
    }
    let missing = state
        .records
        .iter()
        .filter(|(id, _)| !seen.contains_key(*id))
        .map(|(id, record)| (id.clone(), record.clone()))
        .collect::<Vec<_>>();
    for (id, record) in missing {
        let full_path = join(&config.vault_root, &record.path);
        let current = by_id.get(&id).map(|(_, content)| content.as_str());
        if current.is_none() {
            // The authoritative object disappeared. A missing file is already
            // converged; retain durable provenance so the identity can be
            // recovered if the object returns.
            record_provenance(client, &id, "missing", None, None)?;
            continue;
        }
        let current_hash = hash(current.unwrap_or_default());
        if record.conflict.is_none() && current_hash == record.file_hash {
            client.fs("filesystem.delete", &full_path, None)?;
            record_provenance(client, &id, "missing", None, None)?;
        } else {
            record_provenance(client, &id, "conflict", None, Some(&current_hash))?;
            state.records.insert(
                id,
                RecordState {
                    path: record.path,
                    file_hash: current_hash.clone(),
                    ark_hash: record.ark_hash.clone(),
                    conflict: Some(ConflictState {
                        file_hash: current_hash,
                        ark_hash: record.ark_hash,
                    }),
                },
            );
        }
    }
    state.last_sync = Some(chrono::Utc::now().to_rfc3339());
    save_state(client, config, &state)?;
    Ok(BridgeStatus {
        last_sync: state.last_sync,
        conflict_count: state.conflict_count,
        last_conflict_at: state.last_conflict_at,
    })
}

fn main() {
    let stdin = std::io::stdin();
    let mut first = String::new();
    if stdin
        .lock()
        .read_line(&mut first)
        .ok()
        .filter(|read| *read > 0)
        .is_none()
    {
        return;
    }
    let Ok(bootstrap) = serde_json::from_str::<Value>(&first) else {
        return;
    };
    let Some(config) = bootstrap
        .get("bridge_config")
        .cloned()
        .and_then(|value| serde_json::from_value::<Config>(value).ok())
    else {
        return;
    };
    let token = bootstrap
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let generation = bootstrap
        .get("generation")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if Client::send(json!({"method":"worker.hello","package_id":bootstrap["package_id"],"version":bootstrap["version"],"hash":bootstrap["hash"],"pid":bootstrap["pid"],"api_version":bootstrap["api_version"],"token":token})).is_err() { return; }
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if let Ok(value) = serde_json::from_str(&line) {
                if sender.send(value).is_err() {
                    break;
                }
            }
        }
    });
    let mut client = Client {
        token,
        generation,
        next_id: 1,
        inbound: receiver,
        stopped: false,
    };
    let mut status = sync(&mut client, &config).ok();
    client.heartbeat(status.as_ref());
    loop {
        if client.stopped {
            break;
        }
        match client.inbound.recv_timeout(TICK) {
            Ok(message) if message.get("method").and_then(Value::as_str) == Some("worker.stop") => {
                break
            }
            Ok(message) if message.get("method").and_then(Value::as_str) == Some("worker.tick") => {
                status = sync(&mut client, &config).ok().or(status);
                client.heartbeat(status.as_ref());
            }
            Ok(_) | Err(RecvTimeoutError::Timeout) => {
                status = sync(&mut client, &config).ok().or(status);
                client.heartbeat(status.as_ref());
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        Config {
            vault_root: "C:\\vault".into(),
            state_root: "C:\\state".into(),
            selected_types: vec!["com.kosmos.note".into()],
            editable_fields: vec!["title".into(), "body".into()],
            readonly_fields: vec![],
        }
    }
    #[allow(clippy::unwrap_used)]
    #[test]
    fn format_keeps_stable_identity_and_rejects_other_types() {
        let object = json!({"id":"id-1","type_id":"com.kosmos.note","type_version":"1.0.0","title":"Hello","props_json":{"description":null,"extensions":{}},"content_json":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"Body"}]}]}});
        let markdown = render(&object, &config()).unwrap();
        assert!(markdown.contains("ark_id: \"id-1\""));
        assert!(!markdown.contains("secret"));
        assert_eq!(parse(&markdown, &config()).unwrap().0, "id-1");
        assert!(parse("---\nark_id: \"a\"\nark_type: \"other\"\n---\nx", &config()).is_none());
    }

    #[allow(clippy::unwrap_used)]
    #[test]
    fn import_requires_version_and_editable_allowlist() {
        let valid = "---\nark_id: \"id-1\"\nark_type: \"com.kosmos.note\"\nark_version: \"1.0.0\"\nbridge_version: 1\nreadonly: \"changed\"\n---\nbody";
        let parsed = parse(valid, &config()).expect("valid bridge file");
        assert!(parsed.2.is_empty());
        assert!(apply_editable(
            json!({"id":"id-1","type_id":"note","title":"One","props_json":{"body":"body"}}),
            parsed.2,
            parsed.3,
            &config()
        )
        .is_none());
        assert!(parse(
            &valid.replace("bridge_version: 1", "bridge_version: 2"),
            &config()
        )
        .is_none());
        let body_config = Config {
            editable_fields: vec!["title".into()],
            ..config()
        };
        let parsed = parse(valid, &body_config).unwrap();
        assert!(apply_editable(
            json!({"id":"id-1","type_id":"note","title":"One","props_json":{"body":"body"}}),
            parsed.2,
            "different".into(),
            &body_config
        )
        .is_none());
    }

    #[test]
    fn legacy_reserved_alias_is_normalized_at_import_boundary() {
        let markdown = "---\nark_id: \"legacy-1\"\nark_type: \"note_obj\"\nark_version: \"1.0.0\"\nbridge_version: 1\n---\nbody";
        let (id, kind, _, body) = parse(markdown, &config()).expect("legacy alias accepted");
        assert_eq!(id, "legacy-1");
        assert_eq!(kind, "com.kosmos.note");
        assert_eq!(body, "body");
    }

    #[test]
    fn reserved_wrong_version_is_rejected_before_edit() {
        let markdown = "---\nark_id: \"note-1\"\nark_type: \"com.kosmos.note\"\nark_version: \"2.0.0\"\nbridge_version: 1\n---\nchanged";
        assert!(parse(markdown, &config()).is_none());
    }
    #[test]
    fn collision_names_are_deterministic_and_distinct() {
        let first = collision_safe_name("Same", "abcdefghijkl-1");
        let second = collision_safe_name("Same", "abcdefghijkl-2");
        assert_ne!(first, second);
        assert_eq!(first, collision_safe_name("Same", "abcdefghijkl-1"));
    }

    #[test]
    fn collision_key_matches_windows_case_insensitivity() {
        assert_eq!(
            collision_key("Foo", "abcdefghijkl-1"),
            collision_key("foo", "abcdefghijkl-2")
        );
    }

    #[allow(clippy::unwrap_used)]
    #[test]
    fn canonical_body_survives_import_and_export_byte_roundtrip() {
        let object = json!({"id":"note-1","type_id":"com.kosmos.note","type_version":"1.0.0","title":"One","props_json":{"description":null,"extensions":{}},"content_json":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"From ARK"}]}]}});
        let markdown = render(&object, &config()).unwrap();
        let edited = markdown.replace("From ARK", "From server");
        let (id, kind, fields, body) = parse(&edited, &config()).unwrap();
        assert_eq!(id, "note-1");
        assert_eq!(kind, "com.kosmos.note");
        assert_eq!(body, "From server");
        let imported = apply_editable(object, fields, body, &config()).expect("body edit");
        let exported = render(&imported, &config()).unwrap();
        assert!(exported.contains("From server"));
        assert_eq!(exported, edited);
        assert_eq!(exported, render(&imported, &config()).unwrap());
    }

    #[allow(clippy::unwrap_used)]
    #[test]
    fn parsed_rendered_body_has_no_bridge_terminal_newline() {
        let object = json!({"id":"id-1","type_id":"custom.note","title":"Hello","props_json":{"body":"Body"}});
        let custom_config = Config {
            selected_types: vec!["custom.note".into()],
            ..config()
        };
        let markdown = render(&object, &custom_config).unwrap();
        assert_eq!(parse(&markdown, &custom_config).unwrap().3, "Body");
    }

    #[allow(clippy::unwrap_used)]
    #[test]
    fn canonical_markdown_roundtrip_preserves_paragraph_boundaries() {
        let object = json!({"id":"note-1","type_id":"com.kosmos.note","type_version":"1.0.0","title":"One","props_json":{"description":null},"content_json":{"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"First"}]},{"type":"paragraph","content":[{"type":"text","text":"Second"}]}]}});
        let markdown = render(&object, &config()).unwrap();
        let edited = markdown.replace("First\n\nSecond", "First edited\n\nSecond edited");
        let (_, _, fields, body) = parse(&edited, &config()).unwrap();
        let imported = apply_editable(object, fields, body, &config()).unwrap();
        assert_eq!(
            imported["content_json"]["content"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(render(&imported, &config()).unwrap(), edited);
    }
}
