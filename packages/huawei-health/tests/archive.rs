use base64::{engine::general_purpose::STANDARD, Engine as _};
use kosmos_huawei_health_worker::{next_version, Archive, Error};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;

const ACCOUNT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const HANDLE: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
const ORIGIN: &str = "sportdata-dre.things.dbankcloud.com";

fn wait(_: std::time::Duration) -> kosmos_huawei_health_worker::Result<()> {
    Ok(())
}

#[test]
fn reconstructs_chunks_and_releases_response() {
    let payload = b"Huawei page bytes reconstructed from raw chunks".to_vec();
    let state = Rc::new(RefCell::new((payload.clone(), false)));
    let shared = Rc::clone(&state);
    let mut archive = Archive {
        account: ACCOUNT.into(),
        handle: HANDLE.into(),
        data_origin: ORIGIN.into(),
        site_id: 7,
        wait,
        call: move |operation: &str, params: Value| {
            assert_eq!(operation, "network.fetch");
            let mut state = shared.borrow_mut();
            if params.get("close") == Some(&Value::Bool(true)) {
                state.1 = true;
                return Ok(Value::Null);
            }
            if let Some(offset) = params.get("offset").and_then(Value::as_u64) {
                let offset = offset as usize;
                let length = params["length"].as_u64().unwrap() as usize;
                return Ok(json!({"bytes": STANDARD.encode(&state.0[offset..offset + length])}));
            }
            Ok(json!({"response_handle":"response-1","size":state.0.len(),"chunk_size":7}))
        },
    };
    assert_eq!(archive.fetch("common/test", json!({})).unwrap(), payload);
    assert!(state.borrow().1);
}

#[test]
fn checkpoint_follows_every_persisted_page_chunk() {
    let raw = serde_json::to_vec(&json!({
        "resultCode": 0,
        "currentVersion": 1,
        "detailInfos": [{"value": "x".repeat(128 * 1024)}]
    }))
    .unwrap();
    let terminal = br#"{"resultCode":0,"currentVersion":1}"#.to_vec();
    let state = Rc::new(RefCell::new((
        vec![raw, terminal],
        0usize,
        Vec::<String>::new(),
        Vec::<String>::new(),
        true,
        0usize,
    )));
    let shared = Rc::clone(&state);
    let mut archive = Archive {
        account: ACCOUNT.into(),
        handle: HANDLE.into(),
        data_origin: ORIGIN.into(),
        site_id: 7,
        wait,
        call: move |operation: &str, params: Value| {
            let mut state = shared.borrow_mut();
            match operation {
                "ark.read" => Ok(Value::Null),
                "network.fetch" if params.get("close") == Some(&Value::Bool(true)) => {
                    Ok(Value::Null)
                }
                "network.fetch" if params.get("offset").is_some() => {
                    let index = params["response_handle"]
                        .as_str()
                        .unwrap()
                        .trim_start_matches('h')
                        .parse::<usize>()
                        .unwrap();
                    let offset = params["offset"].as_u64().unwrap() as usize;
                    let length = params["length"].as_u64().unwrap() as usize;
                    Ok(json!({"bytes":STANDARD.encode(&state.0[index][offset..offset + length])}))
                }
                "network.fetch" => {
                    let index = state.1;
                    state.1 += 1;
                    Ok(
                        json!({"response_handle":format!("h{index}"),"size":state.0[index].len(),"chunk_size":262144}),
                    )
                }
                "ark.write" if params["operation"] == "upsert_object" => {
                    state.5 += 1;
                    if state.4 && state.5 == 2 {
                        return Err(Error::Host);
                    }
                    state.2.push("upsert_object".into());
                    state
                        .3
                        .push(params["params"]["object"]["id"].as_str().unwrap().into());
                    Ok(Value::Null)
                }
                "ark.write" => {
                    state.2.push(params["operation"].as_str().unwrap().into());
                    Ok(Value::Null)
                }
                _ => Err(Error::Invalid),
            }
        },
    };
    assert!(archive.stream("point", 1, 1).is_err());
    assert_eq!(state.borrow().3.len(), 1);
    assert!(!state
        .borrow()
        .2
        .iter()
        .any(|operation| operation == "set_sync_kv"));
    let first_id = state.borrow().3[0].clone();

    let state = Rc::new(RefCell::new((
        state.borrow().0.clone(),
        0usize,
        Vec::<String>::new(),
        Vec::<String>::new(),
        false,
        0usize,
    )));
    let shared = Rc::clone(&state);
    let mut archive = Archive {
        account: ACCOUNT.into(),
        handle: HANDLE.into(),
        data_origin: ORIGIN.into(),
        site_id: 7,
        wait,
        call: move |operation: &str, params: Value| {
            let mut state = shared.borrow_mut();
            match operation {
                "ark.read" => Ok(Value::Null),
                "network.fetch" if params.get("close") == Some(&Value::Bool(true)) => {
                    Ok(Value::Null)
                }
                "network.fetch" if params.get("offset").is_some() => {
                    let index = params["response_handle"]
                        .as_str()
                        .unwrap()
                        .trim_start_matches('h')
                        .parse::<usize>()
                        .unwrap();
                    let offset = params["offset"].as_u64().unwrap() as usize;
                    let length = params["length"].as_u64().unwrap() as usize;
                    Ok(json!({"bytes":STANDARD.encode(&state.0[index][offset..offset + length])}))
                }
                "network.fetch" => {
                    let index = state.1;
                    state.1 += 1;
                    Ok(
                        json!({"response_handle":format!("h{index}"),"size":state.0[index].len(),"chunk_size":262144}),
                    )
                }
                "ark.write" => {
                    state.2.push(params["operation"].as_str().unwrap().into());
                    if params["operation"] == "upsert_object" {
                        state
                            .3
                            .push(params["params"]["object"]["id"].as_str().unwrap().into());
                    }
                    Ok(Value::Null)
                }
                _ => Err(Error::Invalid),
            }
        },
    };
    archive.stream("point", 1, 1).unwrap();
    let events = &state.borrow().2;
    let checkpoint = events
        .iter()
        .position(|operation| operation == "set_sync_kv")
        .unwrap();
    assert_eq!(checkpoint, 2);
    assert!(events[..checkpoint]
        .iter()
        .all(|operation| operation == "upsert_object"));
    assert_eq!(state.borrow().3.len(), 3);
    assert_eq!(state.borrow().3[0], first_id);
}

#[test]
fn validates_cursor_pages_and_deletions() {
    assert_eq!(
        next_version(
            &json!({"resultCode":0,"currentVersion":2,"detailInfos":[{}]}),
            1,
            2
        ),
        Ok(Some(2))
    );
    assert_eq!(
        next_version(&json!({"resultCode":0}), 1, 2),
        Err(Error::Invalid)
    );
    assert_eq!(next_version(&json!({"resultCode":0}), 2, 2), Ok(None));
    assert_eq!(
        next_version(
            &json!({"resultCode":0,"currentVersion":1,"detailInfos":[{}]}),
            1,
            1
        ),
        Err(Error::Invalid)
    );
    assert_eq!(
        next_version(
            &json!({"resultCode":0,"currentVersion":2,"deleteInfos":[{}]}),
            1,
            2
        ),
        Ok(Some(2))
    );
    assert_eq!(
        next_version(
            &json!({"resultCode":0,"currentVersion":2,"deleteInfos":{}}),
            1,
            2
        ),
        Err(Error::Invalid)
    );
}

#[test]
fn rejects_target_regression_and_other_accounts() {
    let checkpoint = json!({"account":ACCOUNT,"stream":"point-1","cursor":4,"expected":7});
    let state = Rc::new(RefCell::new(checkpoint.to_string()));
    let shared = Rc::clone(&state);
    let mut archive = Archive {
        account: ACCOUNT.into(),
        handle: HANDLE.into(),
        data_origin: ORIGIN.into(),
        site_id: 7,
        wait,
        call: move |operation: &str, _params: Value| match operation {
            "ark.read" => Ok(Value::String(shared.borrow().clone())),
            _ => Err(Error::Invalid),
        },
    };
    assert_eq!(archive.stream("point", 1, 3), Err(Error::Invalid));

    let other = Rc::new(RefCell::new(
        json!({"account":"other","stream":"point-1","cursor":1,"expected":1}).to_string(),
    ));
    let shared = Rc::clone(&other);
    let mut archive = Archive {
        account: ACCOUNT.into(),
        handle: HANDLE.into(),
        data_origin: ORIGIN.into(),
        site_id: 7,
        wait,
        call: move |operation: &str, _params: Value| match operation {
            "ark.read" => Ok(Value::String(shared.borrow().clone())),
            _ => Err(Error::Invalid),
        },
    };
    assert_eq!(archive.stream("point", 1, 1), Err(Error::Invalid));
}

#[test]
fn stop_after_chunk_does_not_attempt_close() {
    let calls = Rc::new(RefCell::new(Vec::<String>::new()));
    let shared = Rc::clone(&calls);
    let mut archive = Archive {
        account: ACCOUNT.into(),
        handle: HANDLE.into(),
        data_origin: ORIGIN.into(),
        site_id: 7,
        wait,
        call: move |operation: &str, params: Value| {
            assert_eq!(operation, "network.fetch");
            let mut calls = shared.borrow_mut();
            if params.get("close") == Some(&Value::Bool(true)) {
                calls.push("close".into());
                return Ok(Value::Null);
            }
            if params.get("offset").and_then(Value::as_u64) == Some(0) {
                calls.push("chunk".into());
                return Ok(json!({"bytes": STANDARD.encode(b"abcd")}));
            }
            if params.get("offset").is_none() {
                calls.push("open".into());
                return Ok(json!({"response_handle":"stop","size":8,"chunk_size":4}));
            }
            Err(Error::Stopped)
        },
    };
    assert_eq!(archive.fetch("common/test", json!({})), Err(Error::Stopped));
    assert_eq!(&*calls.borrow(), &["open", "chunk"]);
}

#[test]
fn stop_during_busy_retry_uses_no_extra_host_call() {
    fn stop_wait(_: std::time::Duration) -> kosmos_huawei_health_worker::Result<()> {
        Err(Error::Stopped)
    }
    let calls = Rc::new(RefCell::new(Vec::<String>::new()));
    let shared = Rc::clone(&calls);
    let busy = br#"{"resultCode":1102}"#;
    let mut archive = Archive {
        account: ACCOUNT.into(),
        handle: HANDLE.into(),
        data_origin: ORIGIN.into(),
        site_id: 7,
        wait: stop_wait,
        call: move |operation: &str, params: Value| {
            assert_eq!(operation, "network.fetch");
            let mut calls = shared.borrow_mut();
            if params.get("close") == Some(&Value::Bool(true)) {
                calls.push("close".into());
                return Ok(Value::Null);
            }
            if params.get("offset").is_some() {
                calls.push("chunk".into());
                return Ok(json!({"bytes": STANDARD.encode(busy)}));
            }
            calls.push("open".into());
            Ok(json!({"response_handle":"busy","size":busy.len(),"chunk_size":262144}))
        },
    };
    assert_eq!(archive.fetch("common/test", json!({})), Err(Error::Stopped));
    assert_eq!(&*calls.borrow(), &["open", "chunk", "close"]);
}

#[test]
fn routes_nondefault_origin_and_site() {
    let state = Rc::new(RefCell::new(None::<Value>));
    let shared = Rc::clone(&state);
    let body = br#"{"resultCode":0}"#;
    let mut archive = Archive {
        account: ACCOUNT.into(),
        handle: HANDLE.into(),
        data_origin: "healthdata.dbankcloud.cn".into(),
        site_id: 42,
        wait,
        call: move |operation: &str, params: Value| {
            assert_eq!(operation, "network.fetch");
            if params.get("close") == Some(&Value::Bool(true)) {
                return Ok(Value::Null);
            }
            if params.get("offset").is_some() {
                return Ok(json!({"bytes": STANDARD.encode(body)}));
            }
            *shared.borrow_mut() = Some(params);
            Ok(json!({"response_handle":"route","size":body.len(),"chunk_size":262144}))
        },
    };
    archive.fetch("common/test", json!({})).unwrap();
    let request = state.borrow().clone().unwrap();
    assert_eq!(
        request["url"],
        "https://healthdata.dbankcloud.cn/dataQuery/common/test"
    );
    assert_eq!(request["body"]["siteId"], "42");
}

#[test]
fn sync_requests_all_confirmed_stream_groups() {
    let sync_requests = Rc::new(RefCell::new(Vec::<Value>::new()));
    let responses = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
    let shared_sync_requests = Rc::clone(&sync_requests);
    let shared_responses = Rc::clone(&responses);
    let mut archive = Archive {
        account: ACCOUNT.into(),
        handle: HANDLE.into(),
        data_origin: ORIGIN.into(),
        site_id: 7,
        wait,
        call: move |operation: &str, params: Value| match operation {
            "ark.read" | "ark.write" => Ok(Value::Null),
            "network.fetch" => {
                if params.get("close") == Some(&Value::Bool(true)) {
                    return Ok(Value::Null);
                }
                if let Some(handle) = params["response_handle"].as_str() {
                    let index = handle.trim_start_matches('h').parse::<usize>().unwrap();
                    let offset = params["offset"].as_u64().unwrap() as usize;
                    let length = params["length"].as_u64().unwrap() as usize;
                    return Ok(json!({
                        "bytes": STANDARD.encode(&shared_responses.borrow()[index][offset..offset + length])
                    }));
                }
                let url = params["url"].as_str().unwrap();
                let raw = if url.ends_with("common/getSyncVersions") {
                    let body = params["body"].clone();
                    shared_sync_requests.borrow_mut().push(body.clone());
                    let versions = body["syncKeys"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|key| json!({"type": key["type"], "version": 0}))
                        .collect::<Vec<_>>();
                    serde_json::to_vec(&json!({"resultCode": 0, "versions": versions})).unwrap()
                } else {
                    assert!(url.ends_with("report/getPersonalReport"));
                    br#"{"resultCode":0}"#.to_vec()
                };
                let index = shared_responses.borrow().len();
                shared_responses.borrow_mut().push(raw.clone());
                Ok(
                    json!({"response_handle": format!("h{index}"), "size": raw.len(), "chunk_size": raw.len()}),
                )
            }
            _ => Err(Error::Invalid),
        },
    };

    archive.sync().unwrap();
    let requests = sync_requests.borrow();
    assert_eq!(requests.len(), 4);
    let ids = |request: &Value| {
        request["syncKeys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| key["type"].as_u64().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&requests[0]), vec![1, 2, 7, 9, 11, 12, 13, 16, 19]);
    assert_eq!(ids(&requests[1]).len(), 68);
    assert!(ids(&requests[1]).contains(&10006));
    for extra in [500021, 500023, 500024, 500026] {
        assert!(ids(&requests[1]).contains(&extra));
    }
    assert_eq!(ids(&requests[2]).len(), 39);
    assert!(ids(&requests[2]).contains(&700013));
    let statistics = ids(&requests[3]);
    assert_eq!(statistics.len(), 63);
    assert!(!statistics.contains(&200005));
    assert!(!statistics.contains(&300002));
    assert!(statistics.contains(&800003));
    assert!(requests[3]["syncKeys"]
        .as_array()
        .unwrap()
        .iter()
        .all(|key| key["category"] == "SampleStatistic"));
}
