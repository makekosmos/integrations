use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::Utc;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{cmp::min, thread, time::Duration};

pub const DATA_ORIGINS: &[&str] = &[
    "healthdata.dbankcloud.cn",
    "sportdata-dra.things.dbankcloud.com",
    "sportdata-dre.things.dbankcloud.com",
    "sportdata-drru.things.dbankcloud.ru",
];
pub const TYPE_ID: &str = "com.kosmos.huawei-health.archive";
pub const MAX_PAGE: usize = 32 * 1024 * 1024;
const ARCHIVE_CHUNK: usize = 128 * 1024;
const ARCHIVE_SCHEMA: &str = include_str!("../schemas/archive.schema.json");
const MAX_DOWNLOAD_CHUNK: usize = 262_144;
const BUSY_RETRY_DELAYS: &[u64] = &[5, 15, 30];
// Type IDs from APK 16.1.6.320 assets/dict_config.txt; categories 0 and 1.
const POINT: &[u64] = &[
    400011, 400012, 400013, 400014, 400015, 500035, 500043, 500036, 500037, 500038, 500039, 500040,
    500041, 500045, 500046, 500047, 500031, 500032, 500033, 300014, 400016, 400025, 400026, 500001,
    600001, 600002, 10002, 10006, 500002, 500027, 500029, 600003, 200001, 200002, 500006, 500007,
    500010, 200003, 200004, 200005, 500018, 500019, 500025, 400019, 500005, 500034, 400017, 400023,
    500013, 500012, 500015, 500014, 400018, 400020, 300002, 300003, 300004, 500030, 500044, 500048,
    500050, 500051, 500052, 500055,
];
// Additional point dictionaries from assets/dict_config.json. These are not
// present in dict_config.txt but return valid point pages for this account.
const EXTRA_POINT: &[u64] = &[500021, 500023, 500024, 500026];
const SEQUENCE: &[u64] = &[
    700001, 700014, 700017, 700004, 700019, 700021, 700022, 700009, 700013, 30287, 700015, 700016,
    700018, 30288, 30289, 30291, 30292, 34260, 34266, 34259, 34265, 34228, 700011, 700012, 30290,
    30223, 30227, 30228, 30229, 33259, 33260, 700002, 700003, 700005, 700006, 700007, 700008,
    700010, 700023,
];
// Base streams requested by Huawei's Llhe.a() sync builder, including the
// three non-dictionary streams that are easy to miss in dict_config.txt.
const LEGACY: &[u64] = &[1, 2, 7, 9, 11, 12, 13, 16, 19];
// Statistics dictionaries use a different endpoint. 800003 (sleep summary)
// is declared only in dict_config.json and must not be sent to the point API.
const EXTRA_STATISTICS: &[u64] = &[800003];

#[derive(Debug, PartialEq)]
pub enum Error {
    Host,
    Invalid,
    Stopped,
}
pub type Result<T> = std::result::Result<T, Error>;

pub fn next_version(page: &Value, previous: u64, expected: u64) -> Result<Option<u64>> {
    if page["resultCode"] != 0 {
        return Err(Error::Invalid);
    }
    let mut count = 0;
    let mut present = 0;
    for field in ["data", "statisticTotal", "detailInfos"] {
        if let Some(value) = page.get(field) {
            present += 1;
            count += if field == "detailInfos" {
                value.as_array().ok_or(Error::Invalid)?.len()
            } else {
                value
                    .as_object()
                    .ok_or(Error::Invalid)?
                    .values()
                    .map(|v| v.as_array().map(Vec::len).ok_or(Error::Invalid))
                    .collect::<Result<Vec<_>>>()?
                    .iter()
                    .sum()
            };
        }
    }
    if present > 1 {
        return Err(Error::Invalid);
    }
    if let Some(deleted) = page.get("deleteInfos") {
        count += deleted.as_array().ok_or(Error::Invalid)?.len();
    }
    let version = match page.get("currentVersion") {
        None => None,
        Some(v) => Some(v.as_u64().ok_or(Error::Invalid)?),
    };
    if version.is_some_and(|v| v > previous) {
        return Ok(version);
    }
    if count == 0 && previous >= expected && version.is_none_or(|v| v == 0 || v == previous) {
        return Ok(None);
    }
    Err(Error::Invalid)
}

pub struct Archive<F, W> {
    pub call: F,
    pub wait: W,
    pub account: String,
    pub handle: String,
    pub data_origin: String,
    pub site_id: u32,
}

impl<F, W> Archive<F, W>
where
    F: FnMut(&str, Value) -> Result<Value>,
    W: FnMut(Duration) -> Result<()>,
{
    fn write(&mut self, operation: &str, params: Value) -> Result<Value> {
        (self.call)("ark.write", json!({"operation":operation,"params":params}))
    }

    fn checkpoint_key(&self, stream: &str) -> String {
        format!("kosmos.integration.huawei.{}.{}", self.account, stream)
    }

    fn checkpoint(&mut self, stream: &str, value: Value) -> Result<()> {
        let key = self.checkpoint_key(stream);
        self.write("set_sync_kv", json!({"key":key,"value":value.to_string()}))?;
        Ok(())
    }

    fn previous(&mut self, stream: &str) -> Result<(u64, u64)> {
        let key = self.checkpoint_key(stream);
        let value = (self.call)(
            "ark.read",
            json!({"operation":"get_sync_kv","params":{"key":key}}),
        )?;
        if value.is_null() {
            return Ok((0, 0));
        }
        let parsed: Value = serde_json::from_str(value.as_str().ok_or(Error::Invalid)?)
            .map_err(|_| Error::Invalid)?;
        if parsed["account"] != self.account || parsed["stream"] != stream {
            return Err(Error::Invalid);
        }
        Ok((
            parsed["cursor"].as_u64().ok_or(Error::Invalid)?,
            parsed["expected"].as_u64().ok_or(Error::Invalid)?,
        ))
    }

    fn fetch_once(&mut self, url: &str, body: Value) -> Result<Vec<u8>> {
        let response = (self.call)(
            "network.fetch",
            json!({"url":url,"body":body,
            "secret_handle":self.handle,"response_mode":"chunks"}),
        )?;
        let handle = response["response_handle"]
            .as_str()
            .ok_or(Error::Invalid)?
            .to_owned();
        let result = (|| {
            let size = usize::try_from(response["size"].as_u64().ok_or(Error::Invalid)?)
                .map_err(|_| Error::Invalid)?;
            let advertised =
                usize::try_from(response["chunk_size"].as_u64().ok_or(Error::Invalid)?)
                    .map_err(|_| Error::Invalid)?;
            let chunk_size = min(advertised, MAX_DOWNLOAD_CHUNK);
            if size == 0 || size > MAX_PAGE || chunk_size == 0 {
                return Err(Error::Invalid);
            }
            let mut bytes = Vec::with_capacity(size);
            while bytes.len() < size {
                let length = chunk_size.min(size - bytes.len());
                let chunk = (self.call)(
                    "network.fetch",
                    json!({"url":url,"response_handle":handle,
                    "offset":bytes.len(),"length":length}),
                )?;
                let decoded = STANDARD
                    .decode(chunk["bytes"].as_str().ok_or(Error::Invalid)?)
                    .map_err(|_| Error::Invalid)?;
                if decoded.len() != length {
                    return Err(Error::Invalid);
                }
                bytes.extend_from_slice(&decoded);
            }
            Ok(bytes)
        })();
        if matches!(&result, Err(Error::Stopped)) {
            return result;
        }
        let close = (self.call)(
            "network.fetch",
            json!({"url":url,"response_handle":handle,"close":true}),
        );
        match (result, close) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(bytes), Ok(_)) => Ok(bytes),
        }
    }

    pub fn fetch(&mut self, path: &str, params: Value) -> Result<Vec<u8>> {
        if !DATA_ORIGINS.contains(&self.data_origin.as_str()) || self.site_id == 0 {
            return Err(Error::Invalid);
        }
        let mut body = json!({"ts":Utc::now().timestamp_millis(),"tokenType":2,"source":1,
            "appId":"com.huawei.health","deviceId":"clientnull","deviceType":"0",
            "upDeviceType":"0","siteId":self.site_id.to_string(),"sysVersion":"Windows","language":"en","isManually":1});
        body.as_object_mut()
            .ok_or(Error::Invalid)?
            .extend(params.as_object().ok_or(Error::Invalid)?.clone());
        let url = format!("https://{}/dataQuery/{path}", self.data_origin);
        for retry in 0..=BUSY_RETRY_DELAYS.len() {
            let raw = self.fetch_once(&url, body.clone())?;
            let busy = serde_json::from_slice::<Value>(&raw)
                .ok()
                .and_then(|page| page["resultCode"].as_i64())
                == Some(1102);
            if !busy {
                return Ok(raw);
            }
            if let Some(delay) = BUSY_RETRY_DELAYS.get(retry) {
                (self.wait)(Duration::from_secs(*delay))?;
                body["ts"] = Utc::now().timestamp_millis().into();
                continue;
            }
            return Ok(raw);
        }
        unreachable!()
    }

    // Immutable content-addressed chunks; stream checkpoints follow all chunks.
    pub fn save_page(&mut self, stream: &str, cursor: u64, raw: &[u8]) -> Result<String> {
        if raw.is_empty() || raw.len() > MAX_PAGE {
            return Err(Error::Invalid);
        }
        let digest = format!("{:x}", Sha256::digest(raw));
        let now = Utc::now().to_rfc3339();
        for (index, chunk) in raw.chunks(ARCHIVE_CHUNK).enumerate() {
            self.write("upsert_object", json!({"object":{
                "id":format!("huawei:{}:{stream}:{cursor}:{digest}:{index}",self.account),
                "typeId":TYPE_ID,"title":"Huawei Health — архив",
                "contentJson":{"encoding":"base64","bytes":STANDARD.encode(chunk)},
                "propsJson":{"source":"huawei-health","account":self.account,"stream":stream,
                    "cursor":cursor,"sha256":digest,"chunk":index,"chunks":raw.len().div_ceil(ARCHIVE_CHUNK),"size":raw.len(),"schemaVersion":1},
                "createdAt":now,"updatedAt":now,"deletedAt":null
            }}))?;
        }
        Ok(digest)
    }

    pub fn stream(&mut self, group: &str, kind: u64, target: u64) -> Result<()> {
        let stream = format!("{group}-{kind}");
        let (mut cursor, prior_target) = self.previous(&stream)?;
        if target < cursor || target < prior_target {
            return Err(Error::Invalid);
        }
        if target == 0 {
            return Ok(());
        }
        let path = match (group, kind) {
            ("legacy", 1) => "sport/getSportsDataByVersion",
            ("legacy", 2) => "path/getMotionPathByVersion",
            ("sequence", _) => "sequence/getSampleSequenceByVersion",
            ("statistics", _) => "health/getHealthStatisticsByVersion",
            _ => "health/getHealthDataByVersion",
        };
        for _ in 0..1000 {
            let mut params = json!({"version":cursor,"dataType":2});
            if !(group == "legacy" && [1, 2].contains(&kind)) {
                params["type"] = kind.into();
            }
            if group == "sequence" {
                params["deviceCode"] = 0.into();
            }
            if group == "statistics" {
                params["dataSource"] = 2.into();
            }
            let raw = self.fetch(path, params)?;
            let page: Value = serde_json::from_slice(&raw).map_err(|_| Error::Invalid)?;
            let next = next_version(&page, cursor, target)?;
            let digest = self.save_page(&stream, cursor, &raw)?;
            cursor = next.unwrap_or(cursor);
            self.checkpoint(
                &stream,
                json!({"account":self.account,"stream":stream,"cursor":cursor,
                "expected":target,"page":digest,"complete":next.is_none()}),
            )?;
            if next.is_none() {
                return Ok(());
            }
        }
        Err(Error::Invalid)
    }

    pub fn sync(&mut self) -> Result<()> {
        if self.account.len() != 64 || !self.account.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Invalid);
        }
        let now = Utc::now().to_rfc3339();
        self.write("upsert_object_type", json!({"object_type":{"id":TYPE_ID,"name":"Архив Huawei Health",
            "schemaJson":ARCHIVE_SCHEMA,"uiSchemaJson":"{}","createdAt":now,"updatedAt":now,"systemLocked":false}}))?;
        let mut point_kinds = POINT.to_vec();
        point_kinds.extend_from_slice(EXTRA_POINT);
        let mut statistics_kinds = POINT.to_vec();
        statistics_kinds.extend_from_slice(EXTRA_STATISTICS);
        let groups: [(&str, &[u64]); 4] = [
            ("legacy", LEGACY),
            ("point", point_kinds.as_slice()),
            ("sequence", SEQUENCE),
            ("statistics", statistics_kinds.as_slice()),
        ];
        for (group, kinds) in groups {
            let kinds: Vec<_> = kinds
                .iter()
                .copied()
                .filter(|k| group != "statistics" || ![200005, 300002].contains(k))
                .collect();
            let keys: Vec<_> = kinds
                .iter()
                .map(|kind| {
                    let mut key = json!({"type":kind,"dataType":2});
                    if group == "statistics" {
                        key["category"] = "SampleStatistic".into();
                    }
                    key
                })
                .collect();
            let raw = self.fetch("common/getSyncVersions", json!({"syncKeys":keys}))?;
            let page: Value = serde_json::from_slice(&raw).map_err(|_| Error::Invalid)?;
            if page["resultCode"] != 0 {
                return Err(Error::Invalid);
            }
            let versions = page["versions"].as_array().ok_or(Error::Invalid)?;
            let mut seen = std::collections::BTreeSet::new();
            for item in versions {
                let kind = item["type"].as_u64().ok_or(Error::Invalid)?;
                if !kinds.contains(&kind)
                    || !seen.insert(kind)
                    || item["version"].as_u64().is_none()
                {
                    return Err(Error::Invalid);
                }
            }
            if seen.len() != kinds.len() {
                return Err(Error::Invalid);
            }
            self.save_page(&format!("{group}-versions"), 0, &raw)?;
            for item in versions {
                self.stream(
                    group,
                    item["type"].as_u64().ok_or(Error::Invalid)?,
                    item["version"].as_u64().ok_or(Error::Invalid)?,
                )?;
            }
        }
        let raw = self.fetch("report/getPersonalReport", json!({
            "bestItems":["bestRopeSkippingSingleCount","bestRopeSkippingContinuousCount","bestRopeSkippingMaxSpeed1MIN","bestRopeSkippingEnduranceAbility","bestRopeSkippingEnduranceTimeAbility"],
            "accumulatedItems":["accumPerfectGoalAchievedDays"]}))?;
        let report: Value = serde_json::from_slice(&raw).map_err(|_| Error::Invalid)?;
        if report["resultCode"] != 0 {
            return Err(Error::Invalid);
        }
        self.save_page("personal-report", 0, &raw)?;
        self.checkpoint("last_success", json!({"at":Utc::now().to_rfc3339()}))?;
        Ok(())
    }
}

pub fn default_wait(delay: Duration) -> Result<()> {
    thread::sleep(delay);
    Ok(())
}
