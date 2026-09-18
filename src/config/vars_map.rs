use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ops::Deref;

/// 变量表：JSON 里允许字符串/数字/布尔标量，统一字符串化
/// （与脚本 vars 行为一致：`vars.qty = 2` 提取后亦为 "2"）
#[derive(Debug, Clone, Default, Serialize)]
pub struct StringVars(pub BTreeMap<String, String>);

impl Deref for StringVars {
    type Target = BTreeMap<String, String>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de> Deserialize<'de> for StringVars {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let m: BTreeMap<String, serde_json::Value> = BTreeMap::deserialize(d)?;
        let mut out = BTreeMap::new();
        for (k, v) in m {
            let s = match v {
                serde_json::Value::String(s) => s,
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            out.insert(k, s);
        }
        Ok(StringVars(out))
    }
}
