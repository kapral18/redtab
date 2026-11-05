use super::*;

pub(super) fn err_string(e: anyhow::Error) -> String {
    format!("{e:#}")
}

pub(super) fn superseded_post(job: &Job, wanted: &Wanted) -> bool {
    job.post_generation().is_some_and(|generation| {
        wanted
            .post
            .lock()
            .is_ok_and(|post| *post != Some(generation))
    })
}

pub(super) fn background_unwanted(job: &Job, wanted: &Wanted) -> bool {
    match job {
        Job::Experience {
            generation,
            request: crate::reddit::experience::Request::Read { .. },
            ..
        } => wanted
            .experience
            .lock()
            .is_ok_and(|current| *current != Some(*generation)),
        Job::Search {
            generation, query, ..
        } => wanted
            .search_slot(query)
            .lock()
            .is_ok_and(|current| *current != Some(*generation)),
        Job::Feed {
            tab, generation, ..
        } => feed_unwanted(*tab, *generation, wanted),
        Job::Group {
            subreddit,
            generation,
        } => group_unwanted(subreddit, *generation, wanted),
        Job::Icon { subreddit } => icon_unwanted(subreddit, wanted),
        _ => false,
    }
}

/// The Search tab's preview of a row's feed; wanted while requested, whatever tab is active.
pub const PREVIEW_FEED: u64 = u64::MAX;

pub(super) fn feed_unwanted(tab: u64, generation: u64, wanted: &Wanted) -> bool {
    wanted.feeds.lock().is_ok_and(|feeds| {
        feeds.as_ref().is_some_and(|feeds| {
            (tab != PREVIEW_FEED && feeds.active != Some(tab))
                || !feeds
                    .requests
                    .get(&tab)
                    .is_some_and(|requests| requests.contains(&generation))
        })
    })
}

pub(super) fn group_unwanted(name: &str, generation: u64, wanted: &Wanted) -> bool {
    wanted.groups.lock().is_ok_and(|groups| {
        groups.as_ref().is_some_and(|groups| {
            groups
                .get(name)
                .is_none_or(|current| current.is_some_and(|current| current != generation))
        })
    })
}

pub(super) fn icon_unwanted(name: &str, wanted: &Wanted) -> bool {
    wanted
        .icons
        .lock()
        .is_ok_and(|icons| icons.as_ref().is_some_and(|icons| !icons.contains(name)))
}

pub(super) fn skipped_image(job: &Job, wanted: &Wanted) -> Option<Msg> {
    match job {
        Job::Thumb { url } | Job::VideoThumb { url }
            if !wanted.thumbs.lock().is_ok_and(|set| set.contains(url)) =>
        {
            Some(Msg::ThumbSkipped { url: url.clone() })
        }
        Job::Image { url, .. } | Job::VideoFrame { url, .. }
            if !wanted.images.lock().is_ok_and(|set| set.contains(url)) =>
        {
            Some(Msg::ImageSkipped { url: url.clone() })
        }
        _ => None,
    }
}

pub(super) fn run(
    reddit: &Reddit,
    job: &Job,
    wanted: &Wanted,
    progress: &mut dyn FnMut(ListingEvent<'_>),
) -> Msg {
    if let Some(message) = skipped_image(job, wanted) {
        return message;
    }
    match job {
        Job::Experience {
            generation,
            request,
            ..
        } => Msg::Experience {
            generation: *generation,
            result: reddit.experience(request).map_err(err_string),
        },
        Job::Search {
            generation,
            query,
            after,
        } => Msg::Search {
            generation: *generation,
            result: reddit
                .experience(&crate::reddit::experience::Request::Read {
                    query: query.clone(),
                    after: after.clone(),
                })
                .map_err(err_string),
        },
        Job::SortedPost {
            generation,
            permalink,
            sort,
        } => Msg::SortedPost {
            generation: *generation,
            sort: sort.clone(),
            result: reddit
                .sorted_post(permalink, sort)
                .map(Box::new)
                .map_err(err_string),
        },
        Job::Membership {
            generation,
            target,
            join,
        } => Msg::Membership {
            generation: *generation,
            result: reddit.set_membership(target, *join).map_err(err_string),
        },
        Job::Subscriptions { generation } => Msg::Subscriptions {
            generation: *generation,
            result: reddit.subscriptions().map_err(err_string),
        },
        Job::Feed {
            tab,
            generation,
            target,
            query,
            after,
        } => Msg::Feed {
            tab: *tab,
            generation: *generation,
            append: after.is_some(),
            result: query
                .fetch(reddit, target, after.as_deref(), progress)
                .map_err(FeedError::from),
        },
        Job::Group {
            subreddit,
            generation,
        } => Msg::Group {
            generation: *generation,
            subreddit: subreddit.clone(),
            result: FeedQuery::DEFAULT_LISTING
                .fetch(
                    reddit,
                    &FeedTarget::Subreddit(subreddit.clone()),
                    None,
                    progress,
                )
                .map_err(FeedError::from),
        },
        Job::Post {
            generation,
            permalink,
        } => Msg::Post {
            generation: *generation,
            result: reddit.post(permalink).map(Box::new).map_err(err_string),
        },
        Job::MoreReplies {
            generation,
            comment_id,
            cursor,
            ..
        } => Msg::MoreReplies {
            generation: *generation,
            comment_id: comment_id.clone(),
            result: reddit.more_replies(cursor).map_err(err_string),
        },
        Job::Image { url, .. } | Job::VideoFrame { url, .. } => {
            let result = if matches!(job, Job::VideoFrame { .. }) {
                fetch_video_frame(reddit, url, job, wanted, MAX_IMAGE_EDGE)
            } else {
                fetch_image(reddit, url, MAX_IMAGE_EDGE)
            };
            if result.is_err()
                && let Some(skipped) = skipped_image(job, wanted)
            {
                return skipped;
            }
            Msg::Image {
                url: url.clone(),
                result: result.map(LoadedImage::from),
            }
        }
        Job::Thumb { url } | Job::VideoThumb { url } => {
            let result = if matches!(job, Job::VideoThumb { .. }) {
                fetch_video_frame(reddit, url, job, wanted, MAX_THUMB_EDGE)
            } else {
                fetch_image(reddit, url, MAX_THUMB_EDGE)
            };
            if result.is_err()
                && let Some(skipped) = skipped_image(job, wanted)
            {
                return skipped;
            }
            Msg::Thumb {
                url: url.clone(),
                result,
            }
        }
        Job::Icon { subreddit } => Msg::Icon {
            subreddit: subreddit.clone(),
            result: reddit.icon(subreddit).map_err(err_string),
        },
        Job::Profile { user } => Msg::Profile {
            user: user.clone(),
            result: reddit.user_profile(user).map_err(err_string),
        },
        Job::ResolveMedia { permalink } => Msg::MediaResolved {
            permalink: permalink.clone(),
            result: reddit
                .post(permalink)
                .map(|page| page.post.media)
                .map_err(err_string),
        },
        Job::DownloadImages { permalink, urls } => Msg::ImagesDownloaded {
            permalink: permalink.clone(),
            result: download_images(reddit, urls).map_err(err_string),
        },
    }
}

pub(super) fn fetch_video_frame(
    reddit: &Reddit,
    url: &str,
    job: &Job,
    wanted: &Wanted,
    max_edge: u32,
) -> Result<DynamicImage, String> {
    let source = reddit.media_url(url).map_err(err_string)?;
    crate::video::first_frame_preview(&source, || {
        wanted.stopped.load(Ordering::Acquire) || skipped_image(job, wanted).is_some()
    })
    .map(|image| {
        if image.width() > max_edge || image.height() > max_edge {
            image.thumbnail(max_edge, max_edge)
        } else {
            image
        }
    })
}

/// Download and decode an image, shrunk to fit `max_edge`. A GIF gives its first frame
/// and the rest of the file is never read: animated GIFs play as video and can be huge.
pub(super) fn fetch_image(
    reddit: &Reddit,
    url: &str,
    max_edge: u32,
) -> Result<DynamicImage, String> {
    reddit
        .still_image(url)
        .map(|img| {
            if img.width() > max_edge || img.height() > max_edge {
                img.thumbnail(max_edge, max_edge)
            } else {
                img
            }
        })
        .map_err(err_string)
}
