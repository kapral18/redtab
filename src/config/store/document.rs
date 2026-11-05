use super::*;

pub(in crate::config) fn patch(original: &str, before: &Config, after: &Config) -> Result<String> {
    // A blank line separates a file header from field or section annotations.
    let mut leading = 0;
    let mut header_end = 0;
    let mut comments = false;
    for line in original.split_inclusive('\n') {
        if !line.trim().is_empty() && !line.trim_start().starts_with('#') {
            break;
        }
        leading += line.len();
        if line.trim_start().starts_with('#') {
            comments = true;
        } else if comments && header_end == 0 {
            header_end = leading;
        }
    }
    if leading == original.len() || header_end == 0 {
        header_end = leading;
    }
    let (header, body) = original.split_at(header_end);
    let mut document: DocumentMut = body
        .parse()
        .context("Invalid TOML; file was left untouched")?;
    let before = before.entries();
    let after = after.entries();
    let keys: std::collections::BTreeSet<_> = before.keys().chain(after.keys()).collect();
    for key in keys {
        if before.get(key) == after.get(key) {
            continue;
        }
        if !after.contains_key(key) {
            remove(document.as_item_mut(), &key.split('.').collect::<Vec<_>>());
            continue;
        }
        let mut item = document.as_item_mut();
        let mut parts = key.split('.').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_some() && item.get(part).is_none() {
                item[part] = Item::Table(Table::new());
            }
            item = &mut item[part];
        }
        let mut value: Value = after[key].to_string().parse()?;
        if let Some(old) = item.as_value() {
            preserve_format(old, &mut value);
        }
        *item = Item::Value(value);
    }
    let body = document.to_string();
    let separator = if !header.is_empty() && !header.ends_with('\n') && !body.is_empty() {
        "\n"
    } else {
        ""
    };
    Ok(format!("{header}{separator}{body}"))
}

pub(super) fn remove(item: &mut Item, parts: &[&str]) {
    let Some(table) = item.as_table_like_mut() else {
        return;
    };
    if parts.len() == 1 {
        table.remove(parts[0]);
        return;
    }
    if let Some(child) = table.get_mut(parts[0]) {
        remove(child, &parts[1..]);
        if child.as_table_like().is_some_and(|t| t.is_empty()) {
            table.remove(parts[0]);
        }
    }
}

/// Keep annotations inside edited arrays, including those on removed elements.
pub(super) fn preserve_format(old: &Value, next: &mut Value) {
    *next.decor_mut() = old.decor().clone();
    let (Some(previous), Some(values)) = (old.as_array(), next.as_array()) else {
        return;
    };
    let mut array = previous.clone();
    for (index, value) in values.iter().enumerate() {
        if index < array.len() {
            if array.get(index).and_then(Value::as_str) != value.as_str() {
                array.replace(index, value.clone());
            }
        } else {
            array.push(value.clone());
        }
    }
    let mut orphaned = String::new();
    while array.len() > values.len() {
        let removed = array.remove(values.len());
        for raw in [removed.decor().prefix(), removed.decor().suffix()]
            .into_iter()
            .flatten()
        {
            orphaned.push_str(raw.as_str().unwrap_or_default());
        }
    }
    if !orphaned.is_empty() {
        orphaned.push_str(array.trailing().as_str().unwrap_or_default());
        array.set_trailing(orphaned);
    }
    *next = Value::Array(array);
}

pub(in crate::config) fn parse(path: &Path, text: &str) -> Result<Config> {
    let mut config: Config = toml::from_str(text)
        .map_err(|error| anyhow::anyhow!("Invalid config: {}", error.message()))?;
    config.validate()?;
    config.path = path.to_path_buf();
    Ok(config)
}

pub(in crate::config) fn readonly(path: &Path, metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;
        let writable = std::ffi::CString::new(path.as_os_str().as_bytes())
            .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 });
        metadata.permissions().mode() & 0o222 == 0 || !writable
    }
    #[cfg(not(unix))]
    {
        metadata.permissions().readonly()
    }
}

pub(super) fn resolve(path: &Path) -> Result<PathBuf> {
    let mut path = std::path::absolute(path)?;
    for _ in 0..32 {
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let link = fs::read_link(&path)?;
                path = if link.is_absolute() {
                    link
                } else {
                    path.parent().unwrap().join(link)
                };
            }
            Ok(_) => return fs::canonicalize(path).map_err(Into::into),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(
                    fs::canonicalize(path.parent().unwrap())?.join(path.file_name().unwrap())
                );
            }
            Err(error) => return Err(error.into()),
        }
    }
    bail!("Config symlink cycle or excessive nesting")
}
