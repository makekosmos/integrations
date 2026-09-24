use super::*;
use chrono::Utc;
use kosmos_codewars_worker::{
    challenge_url, completion_is_new_enough, completion_object, completions_url, cutoff, item_id,
    needs_rank_backfill, page_items, page_reached_cutoff, profile_object, profile_url,
    valid_username, CODING_PROFILE_TYPE_ID, CODING_SUBMISSION_TYPE_ID, MAX_ITEMS,
};

fn json(client: &mut Client<'_>, url: String) -> Result<Value, WorkerError> {
    serde_json::from_slice(&client.fetch(url)?).map_err(|_| WorkerError::Json)
}

pub(super) fn run(client: &mut Client<'_>, username: &str) -> Result<(), WorkerError> {
    let profile = json(client, profile_url(username))?;
    let canonical = profile
        .get("username")
        .and_then(Value::as_str)
        .filter(|value| valid_username(value))
        .ok_or(WorkerError::Invalid)?;
    let existing = client.ark_read("list_objects", Value::Null)?;
    let cutoff = if needs_rank_backfill(&existing, canonical) {
        None
    } else {
        cutoff(client.sync_value()?.as_deref())
    };
    let mut completions = Vec::new();
    let mut page = 0_u64;
    loop {
        let body = json(client, completions_url(canonical, page))?;
        let items = page_items(&body)?;
        let reached = page_reached_cutoff(items, cutoff);
        if completions.len().saturating_add(items.len()) > MAX_ITEMS {
            return Err(WorkerError::Invalid);
        }
        completions.extend(
            items
                .iter()
                .filter(|item| completion_is_new_enough(item, cutoff))
                .cloned(),
        );
        let total_pages = body.get("totalPages").and_then(Value::as_u64).unwrap_or(0);
        if items.is_empty() || reached || page + 1 >= total_pages || page >= 10_000 {
            break;
        }
        page += 1;
    }
    let now = Utc::now().to_rfc3339();
    for (id, name) in [
        (CODING_PROFILE_TYPE_ID, "Профиль программиста"),
        (CODING_SUBMISSION_TYPE_ID, "Отправка задачи"),
    ] {
        client.ark_write("upsert_object_type", json!({"object_type":{"id":id,"name":name,"schemaJson":"{}","uiSchemaJson":"{}","createdAt":now,"updatedAt":now,"systemLocked":false}}))?;
    }
    client.ark_write(
        "upsert_object",
        json!({"object":profile_object(&profile, &now)?}),
    )?;
    for completion in completions {
        let challenge = item_id(&completion).ok_or(WorkerError::Invalid)?;
        let rank = json(client, challenge_url(&challenge))?
            .get("rank")
            .cloned()
            .unwrap_or(Value::Null);
        client.ark_write(
            "upsert_object",
            json!({"object":completion_object(canonical, &completion, &rank)?}),
        )?;
    }
    client.ark_write("set_sync_kv", json!({"key":SYNC_KEY,"value":now}))?;
    Ok(())
}
