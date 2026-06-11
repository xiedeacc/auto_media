use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Debug, Default, Serialize, Deserialize)]
struct TopicCache {
    #[serde(default)]
    xhs: BTreeMap<String, Value>,
    #[serde(default)]
    zhihu: BTreeMap<String, ZhihuTopicEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZhihuTopicEntry {
    pub id: String,
    pub name: String,
}

pub fn get_xhs(path: &Path, name: &str) -> Result<Option<Value>> {
    let cache = load(path)?;
    Ok(cache.xhs.get(&normalize_key(name)).cloned())
}

pub fn set_xhs(path: &Path, name: &str, value: &Value) -> Result<()> {
    let mut cache = load(path)?;
    cache.xhs.insert(normalize_key(name), value.clone());
    save(path, &cache)
}

pub fn get_zhihu(path: &Path, name: &str) -> Result<Option<ZhihuTopicEntry>> {
    let cache = load(path)?;
    Ok(cache.zhihu.get(&normalize_key(name)).cloned())
}

pub fn set_zhihu(path: &Path, name: &str, id: &str) -> Result<()> {
    let mut cache = load(path)?;
    cache.zhihu.insert(
        normalize_key(name),
        ZhihuTopicEntry {
            id: id.to_string(),
            name: normalize_key(name),
        },
    );
    save(path, &cache)
}

fn load(path: &Path) -> Result<TopicCache> {
    if !path.exists() {
        return Ok(TopicCache::default());
    }
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn save(path: &Path, cache: &TopicCache) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(cache)?;
    fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

fn normalize_key(name: &str) -> String {
    name.trim().trim_start_matches('#').trim().to_string()
}
