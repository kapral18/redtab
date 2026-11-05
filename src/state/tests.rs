use super::*;
use crate::model::Media;

mod bookmarks;
mod load;
mod locking;
mod reads;
mod save;

fn cached(fetched_at: i64) -> Freshness {
    Freshness {
        fetched_at,
        source: Source {
            cached: true,
            ..Source::default()
        },
    }
}

fn post(id: &str, sub: &str) -> Post {
    Post {
        id: id.into(),
        subreddit: sub.into(),
        title: id.into(),
        author: "a".into(),
        flair: None,
        score: None,
        comments: 0,
        created: None,
        permalink: format!("/r/{sub}/comments/{id}/x/"),
        media: Media::None,
        preview_size: None,
        stickied: false,
        nsfw: false,
        spoiler: false,
        original_deleted: false,
    }
}

fn age(path: &Path) {
    let old = std::time::SystemTime::now() - STALE_LOCK * 2;
    std::fs::File::open(path)
        .unwrap()
        .set_modified(old)
        .unwrap();
}

fn ordered_freshness(sequence: u64, fetched_at: i64, cached: bool) -> Freshness {
    Freshness {
        fetched_at,
        source: Source {
            order: Some(crate::cache::RequestOrder {
                generation: [1; 16],
                sequence,
            }),
            observed: Some((sequence, sequence)),
            cached,
        },
    }
}
