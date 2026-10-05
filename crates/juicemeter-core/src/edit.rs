//! Edits to the config file that keep the user's comments and layout.

use crate::config::{config_path, SourceConfig};
use anyhow::Context;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};

fn modify(f: impl FnOnce(&mut DocumentMut) -> anyhow::Result<()>) -> anyhow::Result<()> {
    let path = config_path().context("no config directory")?;
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let mut doc: DocumentMut = text.parse().with_context(|| format!("invalid TOML in {}", path.display()))?;
    f(&mut doc)?;
    let out = doc.to_string();
    // Refuse to write something the loader would reject.
    toml::from_str::<crate::Config>(&out).context("edit would produce an invalid config")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, out).with_context(|| format!("writing {}", path.display()))
}

pub fn set_label(account_id: &str, label: Option<&str>) -> anyhow::Result<()> {
    modify(|doc| {
        let labels = doc.entry("labels").or_insert(Item::Table(Table::new()));
        let labels = labels.as_table_mut().context("`labels` in the config is not a table")?;
        match label {
            Some(l) => labels[account_id] = toml_edit::value(l),
            None => {
                labels.remove(account_id);
            }
        }
        Ok(())
    })
}

pub fn add_sources(sources: &[SourceConfig]) -> anyhow::Result<()> {
    // Serialize through `toml` so field names match the loader exactly.
    #[derive(serde::Serialize)]
    struct Wrap<'a> {
        source: &'a [SourceConfig],
    }
    let new: DocumentMut = toml::to_string(&Wrap { source: sources })?.parse()?;
    let new_tables = new.get("source").and_then(Item::as_array_of_tables).cloned().unwrap_or_default();
    modify(|doc| {
        let existing = doc.entry("source").or_insert(Item::ArrayOfTables(ArrayOfTables::new()));
        let existing = existing.as_array_of_tables_mut().context("`source` in the config is not a list of tables")?;
        for mut t in new_tables.into_iter() {
            // `key = { env = "…" }` on one line
            if let Some(Item::Table(key)) = t.remove("key") {
                let mut inline = key.into_inline_table();
                inline.fmt();
                t.insert("key", toml_edit::value(inline));
            }
            existing.push(t);
        }
        Ok(())
    })
}

/// Set (or with `None`, remove) a top-level setting such as `show` or `pin`.
pub fn set_value(key: &str, value: Option<&str>) -> anyhow::Result<()> {
    modify(|doc| {
        match value {
            Some(v) => doc[key] = toml_edit::value(v),
            None => {
                doc.remove(key);
            }
        }
        Ok(())
    })
}

/// Set a top-level list of strings such as `pins`, removing the keys in `replaces`.
pub fn set_list(key: &str, values: &[String], replaces: &[&str]) -> anyhow::Result<()> {
    modify(|doc| {
        let mut arr = toml_edit::Array::new();
        for v in values {
            arr.push(v.as_str());
        }
        doc[key] = toml_edit::value(arr);
        for k in replaces {
            doc.remove(k);
        }
        Ok(())
    })
}
