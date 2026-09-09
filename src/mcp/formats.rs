//! Concrete syntax tree editing only. Semantic trees are used to compute a diff,
//! never to regenerate an existing document. The final semantic comparison also
//! guards against bugs in a format editor changing an unrelated field.
use anyhow::{Context, Result, bail};
use jsonc_parser::cst::{CstInputValue, CstObject, CstRootNode};
use serde_json::Value;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    Json,
    Jsonc,
    Toml,
    Yaml,
}

pub fn parse(text: &str, format: Format) -> Result<Value> {
    if text.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    let value = match format {
        Format::Json | Format::Jsonc => {
            if format == Format::Json {
                serde_json::from_str::<Value>(text).map_err(|e| {
                    anyhow::anyhow!(
                        "JSON parser: {:?} at line {}, column {}",
                        e.classify(),
                        e.line(),
                        e.column()
                    )
                })?;
            }
            let root = json_root(text)?;
            check_json_duplicates(
                &root
                    .object_value()
                    .context("Configuration root must be an object")?,
            )?;
            root.to_serde_value()
                .context("Missing JSON configuration")?
        }
        Format::Toml => {
            let doc = toml_edit::DocumentMut::from_str(text).map_err(|e| {
                let offset = e.span().map(|s| s.start).unwrap_or(0).min(text.len());
                let line = text[..offset].bytes().filter(|b| *b == b'\n').count() + 1;
                anyhow::anyhow!(
                    "TOML parser: invalid syntax at line {line} (source omitted to protect secrets)"
                )
            })?;
            toml_edit::de::from_str::<Value>(&doc.to_string())
                .context("Cannot interpret TOML value")?
        }
        Format::Yaml => {
            let v: Value = serde_yaml::from_str(text).map_err(|e| {
                let pos = e.location().map(|l| format!("line {}, column {}", l.line(), l.column())).unwrap_or_default();
                anyhow::anyhow!("YAML parser: invalid syntax or duplicate key at {pos} (source omitted to protect secrets)")
            })?;
            v
        }
    };
    if !value.is_object() {
        bail!("Configuration root must be a mapping/object");
    }
    Ok(value)
}

fn json_root(text: &str) -> Result<CstRootNode> {
    CstRootNode::parse(text, &Default::default()).map_err(|e| {
        anyhow::anyhow!(
            "JSONC parser: {}",
            e.to_string().lines().next().unwrap_or("invalid syntax")
        )
    })
}
fn check_json_duplicates(obj: &CstObject) -> Result<()> {
    let mut keys = std::collections::HashSet::new();
    for prop in obj.properties() {
        let name = prop
            .name()
            .context("Missing property name")?
            .decoded_value()?;
        if !keys.insert(name) {
            bail!("JSON parser: duplicate object key");
        }
        if let Some(child) = prop.object_value() {
            check_json_duplicates(&child)?;
        }
    }
    Ok(())
}

pub fn edit(text: &str, format: Format, after: &Value) -> Result<String> {
    let before = parse(text, format)?;
    if before == *after {
        return Ok(text.to_string());
    }
    let out = match format {
        Format::Json | Format::Jsonc => {
            let root = json_root(if text.trim().is_empty() { "{}\n" } else { text })?;
            json_diff(
                &root.object_value().context("Expected object")?,
                &before,
                after,
            )?;
            root.to_string()
        }
        Format::Toml => {
            let mut doc = toml_edit::DocumentMut::from_str(text)?;
            toml_diff(doc.as_table_mut(), &before, after)?;
            doc.to_string()
        }
        Format::Yaml => {
            // Anchors and tags may create cross-entry dependencies. Refuse edits
            // until the editor can prove preservation of their semantics.
            let doc =
                yaml_edit::Document::from_str(if text.trim().is_empty() { "{}\n" } else { text })
                    .map_err(|_| {
                    anyhow::anyhow!("YAML syntax tree parser rejected the configuration")
                })?;
            yaml_diff(
                &doc.as_mapping().context("Expected YAML mapping")?,
                &before,
                after,
            )?;
            doc.to_string()
        }
    };
    if parse(&out, format)? != *after {
        bail!("Format editor verification failed; original configuration preserved");
    }
    Ok(out)
}
fn input(v: &Value) -> CstInputValue {
    match v {
        Value::Null => CstInputValue::Null,
        Value::Bool(b) => CstInputValue::Bool(*b),
        Value::Number(n) => CstInputValue::Number(n.to_string()),
        Value::String(s) => CstInputValue::String(s.clone()),
        Value::Array(a) => CstInputValue::Array(a.iter().map(input).collect()),
        Value::Object(o) => {
            CstInputValue::Object(o.iter().map(|(k, v)| (k.clone(), input(v))).collect())
        }
    }
}
fn json_diff(obj: &CstObject, old: &Value, new: &Value) -> Result<()> {
    let old = old.as_object().context("Expected object")?;
    let new = new.as_object().context("Expected object")?;
    for key in old.keys().filter(|k| !new.contains_key(*k)) {
        if let Some(p) = obj.get(key) {
            p.remove();
        }
    }
    for (key, value) in new {
        if old.get(key) == Some(value) {
            continue;
        }
        if old.get(key).is_some_and(Value::is_object) && value.is_object() {
            json_diff(
                &obj.object_value(key).context("Expected object")?,
                &old[key],
                value,
            )?;
        } else if let Some(prop) = obj.get(key) {
            prop.set_value(input(value));
        } else {
            obj.append(key, input(value));
        }
    }
    Ok(())
}
fn toml_item(v: &Value) -> Result<toml_edit::Item> {
    let wrapper = serde_json::json!({"value": v});
    let doc = toml_edit::ser::to_document(&wrapper)?;
    Ok(doc["value"].clone())
}
fn toml_diff(table: &mut dyn toml_edit::TableLike, old: &Value, new: &Value) -> Result<()> {
    let old = old.as_object().context("Expected TOML table")?;
    let new = new.as_object().context("Expected TOML table")?;
    for key in old.keys().filter(|k| !new.contains_key(*k)) {
        table.remove(key);
    }
    for (key, value) in new {
        if old.get(key) == Some(value) {
            continue;
        }
        if old.get(key).is_some_and(Value::is_object) && value.is_object() {
            let child = table
                .get_mut(key)
                .and_then(|i| i.as_table_like_mut())
                .context("Expected TOML table")?;
            toml_diff(child, &old[key], value)?;
        } else {
            let mut item = toml_item(value)?;
            if let (Some(previous), Some(next)) = (
                table.get(key).and_then(|i| i.as_value()),
                item.as_value_mut(),
            ) {
                *next.decor_mut() = previous.decor().clone();
            }
            table.insert(key, item);
        }
    }
    Ok(())
}
fn yaml_value(v: &Value) -> Result<yaml_edit::YamlNode> {
    let text = serde_json::to_string(&serde_json::json!({"value": v}))?;
    let doc = yaml_edit::Document::from_str(&text)
        .map_err(|_| anyhow::anyhow!("Cannot render YAML value"))?;
    doc.get("value").context("Missing YAML value")
}
fn yaml_diff(map: &yaml_edit::Mapping, old: &Value, new: &Value) -> Result<()> {
    let old = old.as_object().context("Expected YAML mapping")?;
    let new = new.as_object().context("Expected YAML mapping")?;
    for key in old.keys().filter(|k| !new.contains_key(*k)) {
        map.remove(key.as_str());
    }
    for (key, value) in new {
        if old.get(key) == Some(value) {
            continue;
        }
        if old.get(key).is_some_and(Value::is_object) && value.is_object() {
            yaml_diff(
                &map.get_mapping(key.as_str())
                    .context("Expected YAML mapping")?,
                &old[key],
                value,
            )?;
        } else if let (Some(old_array), Some(new_array), Some(seq)) = (
            old.get(key).and_then(Value::as_array),
            value.as_array(),
            map.get_sequence(key.as_str()),
        ) {
            // Continue's named server list: update members in place, preserving
            // comments on other servers. General sequences belong to one entry.
            if old_array
                .iter()
                .chain(new_array)
                .all(|v| v.get("name").is_some_and(Value::is_string))
            {
                for i in (0..old_array.len()).rev() {
                    if !new_array.iter().any(|v| v["name"] == old_array[i]["name"]) {
                        seq.remove(i);
                    }
                }
                let remaining: Vec<_> = old_array
                    .iter()
                    .filter(|v| new_array.iter().any(|n| n["name"] == v["name"]))
                    .collect();
                for (i, old_v) in remaining.iter().enumerate() {
                    let new_v = new_array
                        .iter()
                        .find(|v| v["name"] == old_v["name"])
                        .unwrap();
                    if *old_v != new_v {
                        yaml_diff(
                            &seq.get(i)
                                .and_then(|v| v.as_mapping().cloned())
                                .context("Expected MCP mapping")?,
                            old_v,
                            new_v,
                        )?;
                    }
                }
                for v in new_array
                    .iter()
                    .filter(|v| !old_array.iter().any(|o| o["name"] == v["name"]))
                {
                    seq.push(yaml_value(v)?);
                }
            } else {
                map.set(key.as_str(), yaml_value(value)?);
            }
        } else {
            map.set(key.as_str(), yaml_value(value)?);
        }
    }
    Ok(())
}
