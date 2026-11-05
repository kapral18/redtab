use super::*;

#[derive(Default)]
pub(super) struct MemoryStore {
    pub(super) payload: Option<String>,
    pub(super) read_fails: bool,
    pub(super) write_fails: bool,
}

impl Store for MemoryStore {
    fn read(&self) -> Result<Option<String>> {
        if self.read_fails {
            bail!("Keyring locked")
        }
        Ok(self.payload.clone())
    }
    fn write(&mut self, text: Option<&str>) -> Result<()> {
        if self.write_fails {
            bail!("Keyring locked")
        }
        self.payload = text.map(str::to_owned);
        Ok(())
    }
}

pub(super) fn export(id: &str, name: &str, token: &str, expires: u64) -> String {
    format!(
        "# redtab-connection {}\n.reddit.com\tTRUE\t/\tTRUE\t{expires}\treddit_session\t{token}\n",
        serde_json::json!({"account":{"id":id,"name":name},"source":null})
    )
}

pub(super) fn store_with(entries: &[(&str, &str)], active: Option<&str>) -> MemoryStore {
    MemoryStore {
            payload: Some(serde_json::json!({"version":1,"active":active,"sessions":entries.iter().map(|(id,text)| (id.to_string(),text.to_string())).collect::<BTreeMap<_,_>>()} ).to_string()),
            ..MemoryStore::default()
        }
}

pub(super) fn full_store() -> MemoryStore {
    let entries: BTreeMap<_, _> = (0..32)
        .map(|n| {
            let id = format!("account{n}");
            (
                id.clone(),
                export(&id, &format!("reader{n}"), &format!("token{n}"), 0),
            )
        })
        .collect();
    MemoryStore {
        payload: Some(
            serde_json::json!({"version":1,"active":"account0","sessions":entries}).to_string(),
        ),
        ..MemoryStore::default()
    }
}
