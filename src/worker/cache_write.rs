use super::*;

pub(super) fn cache_result(
    msg: Msg,
    job: &Job,
    cache: Option<&Cache>,
    namespace: &str,
    tx: &SyncSender<Msg>,
    source: Source,
) -> Msg {
    if let Job::Experience {
        request: crate::reddit::experience::Request::Write(_),
        invalidations,
        ..
    } = job
    {
        if let Some(cache) = cache {
            let result = cache.reserve_request().and_then(|order| {
                for key in invalidations.iter().filter_map(Invalidation::request) {
                    cache.store(&Cache::key(namespace, &key), &[], crate::app::now(), order)?;
                }
                Ok(())
            });
            if let Err(error) = result {
                let _ = tx.send(Msg::CacheWarning(format!(
                    "Could not invalidate changed content: {error}"
                )));
            }
        }
        return msg;
    }
    if matches!(msg, Msg::Membership { .. }) {
        if let Some(cache) = cache
            && let Some((key, _)) = job.cache_key(namespace)
        {
            // Reserve after the write finishes: reads begun during it must also lose.
            // Empty bytes are an ordered tombstone, never a valid subscription list.
            let result = cache
                .reserve_request()
                .and_then(|order| cache.store(&key, &[], crate::app::now(), order));
            if let Err(error) = result {
                let _ = tx.send(Msg::CacheWarning(format!(
                    "Could not invalidate subscription cache: {error}"
                )));
            }
        }
        return msg;
    }
    let ok = matches!(
        &msg,
        Msg::Experience {
            result: Ok(crate::reddit::experience::Response::Thread(_)),
            ..
        } | Msg::Subscriptions { result: Ok(_), .. }
            | Msg::Feed { result: Ok(_), .. }
            | Msg::Group { result: Ok(_), .. }
            | Msg::Post { result: Ok(_), .. }
            | Msg::MoreReplies { result: Ok(_), .. }
            | Msg::Image { result: Ok(_), .. }
            | Msg::Thumb { result: Ok(_), .. }
            | Msg::Icon { result: Ok(_), .. }
    );
    // Listing failures retain order so late refusals cannot undo newer access or pages.
    if !ok
        && !matches!(
            msg,
            Msg::Feed { .. } | Msg::FeedFallback { .. } | Msg::Group { .. }
        )
    {
        return msg;
    }
    let fetched_at = crate::app::now();
    if ok
        && let Some(cache) = cache
        && let Some(order) = source.order
        && let Some((key, _)) = job.cache_key(namespace)
    {
        let bytes = match &msg {
            Msg::Experience {
                result: Ok(crate::reddit::experience::Response::Thread(value)),
                ..
            } => {
                let full = matches!(job, Job::Experience { request: crate::reddit::experience::Request::Read { query: crate::reddit::experience::Query::Thread(link), .. }, .. } if crate::reddit::experience::thread_location(link).is_ok_and(|(_, comment)| comment.is_none()));
                if full {
                    serde_json::to_vec(value).ok()
                } else {
                    None
                }
            }
            Msg::Subscriptions {
                result: Ok(value), ..
            } => serde_json::to_vec(value).ok(),
            Msg::Feed {
                result: Ok(value), ..
            }
            | Msg::Group {
                result: Ok(value), ..
            } => serde_json::to_vec(value).ok(),
            Msg::Post {
                result: Ok(value), ..
            } => serde_json::to_vec(value).ok(),
            Msg::MoreReplies {
                result: Ok(value), ..
            } => serde_json::to_vec(value).ok(),
            Msg::Icon {
                result: Ok(value), ..
            } => serde_json::to_vec(value).ok(),
            Msg::Image {
                result: Ok(value), ..
            } => encode_png(&value.image),
            Msg::Thumb {
                result: Ok(value), ..
            } => encode_png(value),
            _ => None,
        };
        if let Some(bytes) = bytes
            && let Err(e) = cache.store(&key, &bytes, fetched_at, order)
        {
            let _ = tx.send(Msg::CacheWarning(format!("Could not save cache: {e}")));
        }
    }
    Msg::Snapshot {
        message: Box::new(msg),
        fetched_at,
        refreshing: false,
        source,
    }
}

pub(super) fn encode_png(value: &DynamicImage) -> Option<Vec<u8>> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    value
        .write_to(&mut bytes, image::ImageFormat::Png)
        .ok()
        .map(|()| bytes.into_inner())
}
