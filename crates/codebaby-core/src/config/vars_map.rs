use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ops::Deref;

/// 单个变量的解析结果
#[derive(Debug, Clone, Serialize)]
pub struct VarValue {
    pub value: String,
    /// 敏感标记：所有输出层（curl 命令/报告/日志）自动脱敏
    pub secret: bool,
}

/// 变量表：JSON 里允许
/// - 字符串/数字/布尔标量（统一字符串化）
/// - 对象形态 {"value": "...", "secret": true}            → 敏感变量
/// - 对象形态 {"from": "env", "name": "TOKEN"}             → 运行时读进程环境变量
///   （{"from":"env"} 时 name 缺省用变量名本身）
#[derive(Debug, Clone, Default, Serialize)]
pub struct StringVars(pub BTreeMap<String, VarValue>);

impl Deref for StringVars {
    type Target = BTreeMap<String, VarValue>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl StringVars {
    /// 解析后纯值视图（旧调用方兼容：只要 value）
    pub fn plain(&self) -> BTreeMap<String, String> {
        self.0.iter().map(|(k, v)| (k.clone(), v.value.clone())).collect()
    }

    /// secret 值清单（输出脱敏用）
    pub fn secret_values(&self) -> Vec<String> {
        self.0.values().filter(|v| v.secret).map(|v| v.value.clone()).collect()
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum VarDef {
    /// 对象形态优先匹配（value/secret/from/name）；
    /// 字符串/数字/布尔等标量不匹配后回落 Plain
    Advanced {
        value: Option<String>,
        secret: Option<bool>,
        from: Option<String>,
        name: Option<String>,
    },
    Plain(serde_json::Value),
}

impl VarDef {
    fn resolve(&self, key: &str) -> Result<VarValue, String> {
        match self {
            VarDef::Plain(v) => Ok(VarValue {
                value: value_to_string(v),
                secret: false,
            }),
            VarDef::Advanced { value, secret, from, name } => {
                if let Some(from) = from {
                    if from != "env" {
                        return Err(format!("变量 {key}: 不支持的 from 来源 {from:?}（当前仅 env）"));
                    }
                    let env_name = name.clone().unwrap_or_else(|| key.to_string());
                    let v = std::env::var(&env_name).map_err(|_| {
                        format!("变量 {key}: 进程环境变量 {env_name} 不存在（from:env）")
                    })?;
                    return Ok(VarValue { value: v, secret: secret.unwrap_or(true) });
                }
                match value {
                    Some(v) => Ok(VarValue { value: v.clone(), secret: secret.unwrap_or(false) }),
                    None => Err(format!("变量 {key}: 对象形态需提供 value 或 from")),
                }
            }
        }
    }
}

fn value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

impl<'de> Deserialize<'de> for StringVars {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let m: BTreeMap<String, VarDef> = BTreeMap::deserialize(d)?;
        let mut out = BTreeMap::new();
        for (k, def) in m {
            let v = def.resolve(&k).map_err(serde::de::Error::custom)?;
            out.insert(k, v);
        }
        Ok(StringVars(out))
    }
}
