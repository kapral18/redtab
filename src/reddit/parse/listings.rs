use super::*;

#[derive(Deserialize)]
pub(super) struct Envelope {
    pub(super) kind: String,
    pub(super) data: ListData,
}

#[derive(Deserialize)]
pub(super) struct ListData {
    pub(super) children: Vec<Thing>,
    #[serde(default)]
    pub(super) after: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct Thing {
    pub(super) kind: String,
    pub(super) data: Value,
}

pub(super) fn listing(value: Value) -> Result<ListData> {
    let envelope: Envelope = serde_json::from_value(value).context("invalid Reddit listing")?;
    if envelope.kind != "Listing" {
        bail!("expected Reddit Listing");
    }
    Ok(envelope.data)
}

#[cfg(test)]
pub fn parse_listing(json: &str) -> Result<Listing> {
    parse_listing_value(serde_json::from_str(json)?)
}

#[cfg(test)]
pub(in crate::reddit) fn parse_listing_value(value: Value) -> Result<Listing> {
    let data = listing(value)?;
    let mut posts = Vec::new();
    for thing in data.children {
        if thing.kind != "t3" {
            continue;
        }
        posts.push(post(thing.data)?.0);
    }
    Ok(Listing {
        posts,
        after: data.after.filter(|s| !s.is_empty()),
    })
}

pub(in crate::reddit) fn stream_listing(
    reader: impl std::io::Read,
    progress: &mut dyn FnMut(&Post),
) -> serde_json::Result<Listing> {
    let mut state = ListingStream {
        listing: Listing::default(),
        kind: None,
        data: false,
        children: false,
        after: false,
        emitted: 0,
        seen: HashSet::new(),
        progress,
    };
    let mut decoder = serde_json::Deserializer::from_reader(std::io::BufReader::new(reader));
    ListingSeed {
        state: &mut state,
        field: ListingField::Envelope,
    }
    .deserialize(&mut decoder)?;
    decoder.end()?;
    Ok(state.listing)
}

pub(super) struct ListingStream<'a> {
    pub(super) listing: Listing,
    pub(super) kind: Option<String>,
    pub(super) data: bool,
    pub(super) children: bool,
    pub(super) after: bool,
    pub(super) emitted: usize,
    pub(super) seen: HashSet<String>,
    pub(super) progress: &'a mut dyn FnMut(&Post),
}

impl ListingStream<'_> {
    pub(super) fn publish(&mut self) {
        if self.kind.as_deref() == Some("Listing") {
            for post in &self.listing.posts[self.emitted..] {
                (self.progress)(post);
            }
            self.emitted = self.listing.posts.len();
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum ListingField {
    Envelope,
    Data,
    Children,
}

pub(super) struct ListingSeed<'a, 'b> {
    pub(super) state: &'a mut ListingStream<'b>,
    pub(super) field: ListingField,
}

impl<'de> DeserializeSeed<'de> for ListingSeed<'_, '_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(
        self,
        decoder: D,
    ) -> std::result::Result<(), D::Error> {
        match self.field {
            ListingField::Children => decoder.deserialize_seq(self),
            _ => decoder.deserialize_map(self),
        }
    }
}

impl<'de> Visitor<'de> for ListingSeed<'_, '_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Reddit listing")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> std::result::Result<(), M::Error> {
        while let Some(key) = map.next_key::<String>()? {
            match (self.field, key.as_str()) {
                (ListingField::Envelope, "kind") => {
                    if self.state.kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    let kind: String = map.next_value()?;
                    if kind != "Listing" {
                        return Err(de::Error::custom("expected Reddit Listing"));
                    }
                    self.state.kind = Some(kind);
                    self.state.publish();
                }
                (ListingField::Envelope, "data") => {
                    if self.state.data {
                        return Err(de::Error::duplicate_field("data"));
                    }
                    self.state.data = true;
                    map.next_value_seed(ListingSeed {
                        state: self.state,
                        field: ListingField::Data,
                    })?;
                }
                (ListingField::Data, "children") => {
                    if self.state.children {
                        return Err(de::Error::duplicate_field("children"));
                    }
                    self.state.children = true;
                    map.next_value_seed(ListingSeed {
                        state: self.state,
                        field: ListingField::Children,
                    })?;
                }
                (ListingField::Data, "after") => {
                    if self.state.after {
                        return Err(de::Error::duplicate_field("after"));
                    }
                    self.state.after = true;
                    self.state.listing.after = map
                        .next_value::<Option<String>>()?
                        .filter(|s| !s.is_empty());
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        match self.field {
            ListingField::Envelope if self.state.kind.is_none() || !self.state.data => {
                Err(de::Error::custom("incomplete Reddit listing"))
            }
            ListingField::Data if !self.state.children => Err(de::Error::missing_field("children")),
            _ => Ok(()),
        }
    }

    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> std::result::Result<(), S::Error> {
        while let Some(thing) = seq.next_element::<Thing>()? {
            if thing.kind == "t3" {
                let post = post(thing.data)
                    .map_err(|_| de::Error::custom("invalid Reddit post"))?
                    .0;
                if self.state.seen.insert(post.id.clone()) {
                    self.state.listing.posts.push(post);
                    self.state.publish();
                }
            }
        }
        Ok(())
    }
}

pub(in crate::reddit) fn parse_subscriptions(
    value: Value,
) -> Result<(Vec<Subscription>, Option<String>)> {
    let data = listing(value)?;
    let mut names = Vec::new();
    for thing in data.children {
        if thing.kind != "t5" {
            bail!("expected Reddit subreddit in subscriptions");
        }
        let profile = thing.data["subreddit_type"] == "user"
            || thing.data["display_name_prefixed"]
                .as_str()
                .is_some_and(|name| name.starts_with("u/"))
            || thing.data["url"]
                .as_str()
                .and_then(crate::model::profile_user)
                .is_some();
        if profile {
            if thing.data["subreddit_type"]
                .as_str()
                .is_some_and(|kind| kind != "user")
            {
                bail!("conflicting Reddit subscription type");
            }
            let canonical = thing.data["url"]
                .as_str()
                .and_then(crate::model::profile_user);
            let prefixed = thing.data["display_name_prefixed"]
                .as_str()
                .and_then(|name| name.strip_prefix("u/"));
            let displayed = thing.data["display_name"]
                .as_str()
                .and_then(|name| name.strip_prefix("u_"));
            let name = canonical
                .or(prefixed)
                .or(displayed)
                .filter(|name| crate::model::is_username(name))
                .context("invalid Reddit followed user name")?;
            if [canonical, prefixed, displayed]
                .into_iter()
                .flatten()
                .any(|other| !other.eq_ignore_ascii_case(name))
            {
                bail!("conflicting Reddit followed user identity");
            }
            names.push(Subscription::User {
                name: name.to_string(),
                icon: media_url(&thing.data["icon_img"]),
            });
            continue;
        }
        let name = thing.data["display_name"]
            .as_str()
            .filter(|name| {
                !name.is_empty()
                    && name.len() <= 50
                    && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
            })
            .context("invalid Reddit subscription name")?;
        names.push(Subscription::Community {
            name: name.to_string(),
            icon: community_icon(&thing.data),
        });
    }
    Ok((names, data.after.filter(|s| !s.is_empty())))
}
