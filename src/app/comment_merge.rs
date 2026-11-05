use super::*;

pub(super) fn filter_removed_comments(
    comments: &mut Vec<Comment>,
    source: Source,
    removed: &HashMap<String, Source>,
) {
    comments.retain_mut(|comment| {
        if removed
            .get(&comment.id)
            .is_some_and(|old| !source.may_replace(*old))
        {
            return false;
        }
        filter_removed_comments(&mut comment.replies, source, removed);
        true
    });
}

pub(super) fn record_removal(removed: &mut HashMap<String, Source>, id: &str, source: Source) {
    if removed.get(id).is_none_or(|old| source.may_replace(*old)) {
        removed.insert(id.to_owned(), source);
    }
}

/// Merge partial trees without letting older snapshots replace newer fields or continuations.
pub(super) fn merge_comments(
    incoming: &mut Vec<Comment>,
    previous: &mut Vec<Comment>,
    source: Source,
    keep_missing: bool,
    partial: bool,
    removed: &mut HashMap<String, Source>,
) {
    for comment in incoming.iter_mut() {
        let old = previous.iter_mut().find(|old| old.id == comment.id);
        merge_comment(comment, old, source, partial, removed);
    }
    if previous.is_empty() {
        return;
    }
    let known: HashSet<_> = incoming.iter().map(|comment| comment.id.as_str()).collect();
    previous.retain_mut(|comment| {
        let retained = !known.contains(comment.id.as_str())
            && (keep_missing || comment_newer_than(comment, source));
        if retained
            && !partial
            && let Some(sources) = &mut comment.sources
        {
            sources.parent = Some(parent_membership(sources.parent, source, false));
        }
        if !retained && !known.contains(comment.id.as_str()) {
            record_removal(removed, &comment.id, source);
        }
        retained
    });
    drop(known);
    incoming.append(previous);
}

pub(super) fn parent_membership(
    previous: Option<ParentMembership>,
    source: Source,
    present: bool,
) -> ParentMembership {
    previous
        .filter(|old| !source.may_replace(old.source))
        .unwrap_or(ParentMembership { source, present })
}

pub(super) fn merge_comment(
    incoming: &mut Comment,
    previous: Option<&mut Comment>,
    source: Source,
    partial: bool,
    removed: &mut HashMap<String, Source>,
) {
    let Some(previous) = previous else {
        incoming.sources = Some(CommentSources {
            content: source,
            more: source,
            parent: (!partial).then_some(ParentMembership {
                source,
                present: true,
            }),
        });
        for child in &mut incoming.replies {
            merge_comment(child, None, source, partial, removed);
        }
        return;
    };
    let old = previous.sources.unwrap_or_default();
    let keep_content = !source.may_replace(old.content);
    let keep_more = !source.may_replace(old.more);
    let keep_children = replies_superseded(previous, source);
    let incoming_children = std::mem::take(&mut incoming.replies);
    let mut previous_children = std::mem::take(&mut previous.replies);
    let incoming_more = incoming.more.take();
    let previous_more = previous.more.take();
    let more_source = if keep_more || (partial && incoming_more.is_none()) {
        old.more
    } else {
        source
    };
    let has_more = if keep_more {
        previous_more.is_some()
    } else {
        incoming_more.is_some()
    };
    if keep_content {
        std::mem::swap(incoming, previous);
    }
    if keep_children || keep_content {
        incoming.replies = previous_children;
        merge_replies(
            &mut incoming.replies,
            incoming_children,
            source,
            !keep_children,
            if partial {
                ReplyMerge::Expansion {
                    reorder: false,
                    refreshed: Some(&[]),
                    order: &[],
                }
            } else {
                ReplyMerge::ParentSnapshot
            },
            removed,
        );
    } else {
        incoming.replies = incoming_children;
        merge_comments(
            &mut incoming.replies,
            &mut previous_children,
            source,
            has_more || partial,
            partial,
            removed,
        );
    }
    incoming.more = if keep_more {
        previous_more
    } else if partial {
        crate::reddit::merge_reply_continuations(previous_more, incoming_more, &incoming.replies)
    } else {
        incoming_more
    };
    incoming.sources = Some(CommentSources {
        content: if keep_content { old.content } else { source },
        more: more_source,
        parent: if partial {
            old.parent
        } else {
            Some(parent_membership(old.parent, source, true))
        },
    });
}

pub(super) enum ReplyMerge<'a> {
    ParentSnapshot,
    Expansion {
        reorder: bool,
        refreshed: Option<&'a [String]>,
        order: &'a [String],
    },
}

pub(super) fn merge_replies(
    existing: &mut Vec<Comment>,
    replies: Vec<Comment>,
    source: Source,
    allow_new: bool,
    kind: ReplyMerge<'_>,
    removed: &mut HashMap<String, Source>,
) {
    let expansion = matches!(&kind, ReplyMerge::Expansion { .. });
    let order = match &kind {
        ReplyMerge::Expansion {
            reorder: true,
            order,
            ..
        } if allow_new => Some(*order),
        _ => None,
    };
    // A More response replaces unconfirmed children within its coverage; current
    // parent membership and independently newer content still win.
    if expansion {
        fn protected(comment: &Comment, source: Source) -> bool {
            comment
                .sources
                .is_none_or(|s| s.parent.is_some_and(|p| p.present))
                || comment_newer_than(comment, source)
        }
        fn prune(
            existing: &mut Vec<Comment>,
            returned: &[Comment],
            source: Source,
            ids: Option<&[String]>,
            removed: &mut HashMap<String, Source>,
        ) {
            existing.retain_mut(|comment| {
                let covered = ids.is_none_or(|ids| ids.contains(&comment.id));
                let retained = !covered
                    || protected(comment, source)
                    || find_comment(returned, &comment.id).is_some();
                if !retained {
                    record_removal(removed, &comment.id, source);
                } else if ids.is_some() {
                    // Explicit coverage can name descendants loaded by another cursor.
                    // An unbounded parent response only certifies its direct children.
                    prune(&mut comment.replies, returned, source, ids, removed);
                }
                retained
            });
        }
        let ids = match &kind {
            ReplyMerge::Expansion { refreshed, .. } => *refreshed,
            ReplyMerge::ParentSnapshot => unreachable!(),
        };
        if let Some(ids) = ids {
            for id in ids {
                if find_comment(&replies, id).is_none()
                    && find_comment(existing, id).is_none_or(|c| !protected(c, source))
                {
                    record_removal(removed, id, source);
                }
            }
        }
        prune(existing, &replies, source, ids, removed);
    }
    for mut reply in replies {
        if let Some(at) = existing.iter().position(|old| old.id == reply.id) {
            let old = &mut existing[at];
            merge_comment(&mut reply, Some(old), source, expansion, removed);
            *old = reply;
        } else if allow_new {
            merge_comment(&mut reply, None, source, expansion, removed);
            existing.push(reply);
        }
    }
    if let Some(order) = order {
        // A batch ranks unconfirmed siblings before its continuation, including
        // retained later batches. Parent-confirmed and unranked positions stay put.
        let mut ranks = HashMap::new();
        for (rank, id) in order.iter().enumerate() {
            ranks.entry(id.as_str()).or_insert(rank);
        }
        let slots: Vec<_> = existing
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                ranks.contains_key(c.id.as_str())
                    && c.sources
                        .is_some_and(|s| s.parent.is_none_or(|p| !p.present))
            })
            .map(|(i, _)| i)
            .collect();
        let mut ids: Vec<_> = slots.iter().map(|&i| existing[i].id.clone()).collect();
        ids.sort_by_key(|id| ranks[id.as_str()]);
        for (slot, id) in slots.into_iter().zip(ids) {
            let at = existing
                .iter()
                .position(|c| c.id == id)
                .expect("ranked reply");
            existing.swap(slot, at);
        }
    }
}

/// True when `parent`'s reply list came complete (no More link) from a source that
/// `source` cannot replace: `source` may then neither replace nor add to it. Deeper
/// lists are judged on their own.
pub(super) fn replies_superseded(parent: &Comment, source: Source) -> bool {
    parent.more.is_none()
        && parent
            .sources
            .is_some_and(|old| !source.may_replace(old.more))
}

/// The first reply in `replies` that `previous` lacks, depth first, skipping folded
/// threads and lists `source` may not add to (`allow_new` at this level).
pub(super) fn first_new_reply<'a>(
    replies: &'a [Comment],
    previous: &[Comment],
    source: Source,
    allow_new: bool,
    collapsed: &HashSet<String>,
) -> Option<&'a str> {
    for reply in replies {
        let Some(old) = previous.iter().find(|old| old.id == reply.id) else {
            if allow_new {
                return Some(&reply.id);
            }
            continue;
        };
        if !collapsed.contains(&old.id)
            && let Some(id) = first_new_reply(
                &reply.replies,
                &old.replies,
                source,
                !replies_superseded(old, source),
                collapsed,
            )
        {
            return Some(id);
        }
    }
    None
}

pub(super) fn comment_newer_than(comment: &Comment, source: Source) -> bool {
    comment.sources.is_some_and(|old| {
        !source.may_replace(old.content)
            || !source.may_replace(old.more)
            || old.parent.is_some_and(|p| !source.may_replace(p.source))
    }) || comment
        .replies
        .iter()
        .any(|child| comment_newer_than(child, source))
}

/// Move `current` by `delta` within `0..len`, clamping at the ends.
pub(super) fn step(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    current.saturating_add_signed(delta).min(len - 1)
}
