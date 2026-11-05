use super::*;

impl App {
    pub(super) fn details_menu(&mut self, details: Details) {
        let target = &details.target;
        let mut choices = vec![
            choice(
                "u",
                if details.vote == 1 {
                    "Remove upvote"
                } else {
                    "Upvote"
                },
                Command::Write(Action::Vote {
                    id: target.id.clone(),
                    direction: if details.vote == 1 { 0 } else { 1 },
                }),
            ),
            choice(
                "d",
                if details.vote == -1 {
                    "Remove downvote"
                } else {
                    "Downvote"
                },
                Command::Write(Action::Vote {
                    id: target.id.clone(),
                    direction: if details.vote == -1 { 0 } else { -1 },
                }),
            ),
            choice(
                "s",
                if details.saved {
                    "Unsave on Reddit"
                } else {
                    "Save on Reddit"
                },
                Command::Write(Action::Save {
                    id: target.id.clone(),
                    save: !details.saved,
                }),
            ),
            choice(
                "c",
                "Reply",
                Command::Input(form(
                    "Reply (Markdown)",
                    FormKind::Reply(target.clone()),
                    vec![field("Reply", "", true)],
                    &target.permalink,
                )),
            ),
            read("a", "Author profile", Query::Profile(target.author.clone())),
            read(
                "i",
                if crate::model::profile_user(&target.permalink).is_some() {
                    "Profile information"
                } else {
                    "Community information"
                },
                crate::model::profile_user(&target.permalink).map_or_else(
                    || Query::Community(target.subreddit.clone()),
                    |name| Query::Profile(name.to_string()),
                ),
            ),
            choice(
                "R",
                "Report content",
                Command::Input(form(
                    "Report content to Reddit",
                    FormKind::Report(target.clone()),
                    vec![field("Reason (up to 100 characters)", "", false)],
                    &target.permalink,
                )),
            ),
        ];
        if self.experience.screens.iter().any(|screen| {
            matches!(
                screen,
                Screen::Browse {
                    query: Query::Inbox(_),
                    ..
                }
            )
        }) {
            choices.push(choice(
                "nr",
                "Mark notification read",
                Command::Write(Action::Read {
                    id: target.id.clone(),
                    read: true,
                }),
            ));
            choices.push(choice(
                "nu",
                "Mark notification unread",
                Command::Write(Action::Read {
                    id: target.id.clone(),
                    read: false,
                }),
            ));
        }
        if target.id.starts_with("t3_") {
            choices.push(choice(
                "H",
                if details.hidden {
                    "Unhide post"
                } else {
                    "Hide post"
                },
                Command::Write(Action::Hide {
                    id: target.id.clone(),
                    hide: !details.hidden,
                }),
            ));
            choices.push(choice(
                "f",
                "Follow post notifications",
                Command::Write(Action::FollowPost {
                    id: target.id.clone(),
                    follow: true,
                }),
            ));
            choices.push(choice(
                "F",
                "Unfollow post notifications",
                Command::Write(Action::FollowPost {
                    id: target.id.clone(),
                    follow: false,
                }),
            ));
        }
        if self
            .reddit
            .account()
            .is_some_and(|me| me.name.eq_ignore_ascii_case(&target.author))
        {
            if details.editable {
                choices.push(choice(
                    "e",
                    "Edit my content",
                    Command::Input(form(
                        "Edit (Markdown)",
                        FormKind::Edit(target.clone()),
                        vec![field("Body", &details.raw_text, true)],
                        &target.permalink,
                    )),
                ));
            }
            choices.push(choice(
                "D",
                "Delete my content…",
                Command::Confirm(
                    "Delete this content from Reddit? This cannot be undone.".into(),
                    Action::Delete {
                        target: target.clone(),
                    },
                ),
            ));
        }
        if let Some(path) = details.crosspost {
            choices.push(read("x", "Read original crosspost", Query::Thread(path)));
        }
        let profile = format!("/user/{}", target.author);
        let profile_owner = crate::model::profile_user(&target.permalink);
        let sub = profile_owner.map_or_else(
            || format!("/r/{}", target.subreddit),
            |name| format!("/user/{name}/"),
        );
        choices.extend([
            native(
                "op",
                "Open content, vote in poll or view awards",
                &target.permalink,
            ),
            native(
                "oc",
                "Crosspost or manage post notifications",
                &target.permalink,
            ),
            native("oa", "Author profile: follow, block or chat", &profile),
            native(
                "os",
                if profile_owner.is_some() {
                    "Profile controls"
                } else {
                    "Mute/favorite community or select flair"
                },
                &sub,
            ),
            native("om", "Moderate this content", &target.permalink),
        ]);
        if name(&target.author).is_err() {
            choices.retain(|choice| match &choice.command {
                Command::Read(Query::Profile(_)) => false,
                Command::Native(path) => path != &profile,
                _ => true,
            });
        }
        let kind = if target.comment().is_some() {
            "Comment"
        } else {
            "Post"
        };
        self.push_menu(
            &format!("{kind} · u/{}", target.author),
            choices,
            MenuScope::Content,
        );
        if let Some(Screen::Menu { native, .. }) = self.experience.screens.last_mut() {
            *native = target.permalink.clone();
        }
    }

    pub fn membership_target(&self) -> Option<MembershipTarget> {
        if matches!(self.overlay, Some(Overlay::Experience)) {
            if let Some(Screen::Menu {
                choices, native, ..
            }) = self.experience.screens.last()
            {
                return choices
                    .iter()
                    .find_map(|choice| match &choice.command {
                        Command::Membership(target) => Some(target.clone()),
                        _ => None,
                    })
                    .or_else(|| {
                        native
                            .strip_prefix("/r/")
                            .filter(|name| is_community(name))
                            .map(|name| MembershipTarget::Community(name.into()))
                    });
            }
            let Screen::Browse {
                query,
                page,
                selected,
                ..
            } = self.experience.screens.last()?
            else {
                return None;
            };
            let entry = page.entries.get(*selected);
            if let Some(user) = entry
                .and_then(|entry| entry.target.as_ref())
                .and_then(|target| crate::model::profile_user(&target.permalink))
            {
                return Some(MembershipTarget::User(user.into()));
            }
            let sub = self.experience_community();
            if !sub.is_empty() {
                return Some(MembershipTarget::Community(sub));
            }
            return entry
                .and_then(|entry| entry.query.as_ref())
                .into_iter()
                .chain([query])
                .find_map(|query| match query {
                    Query::Profile(name) | Query::User { name, .. } => {
                        Some(MembershipTarget::User(name.clone()))
                    }
                    _ => None,
                })
                .filter(MembershipTarget::is_valid);
        }
        self.profile_context()
            .map(MembershipTarget::User)
            .or_else(|| {
                self.community()
                    .filter(|name| is_community(name))
                    .map(MembershipTarget::Community)
            })
    }

    pub fn community_menu(&mut self) {
        let Some(target) = self.membership_target() else {
            self.reddit_menu();
            return;
        };
        if let MembershipTarget::User(user) = &target {
            let path = format!("/user/{user}/");
            let action = self
                .subscribed(&target)
                .map_or("Check following status", |joined| target.action(!joined));
            self.push_menu(
                &target.label(),
                vec![
                    choice("m", action, Command::Membership(target.clone())),
                    choice("f", "Open submitted posts", Command::User(user.clone())),
                    choice(
                        "o",
                        "Open profile in browser ↗",
                        Command::Native(path.clone()),
                    ),
                    read("i", "Profile info", Query::Profile(user.clone())),
                ],
                MenuScope::User,
            );
            if let Some(Screen::Menu { native, .. }) = self.experience.screens.last_mut() {
                *native = path;
            }
            return;
        }
        let MembershipTarget::Community(sub) = target else {
            unreachable!()
        };
        let path = format!("/r/{sub}");
        self.push_menu(
            &format!("r/{sub}"),
            vec![
                choice("f", "Open community feed", Command::Subreddit(sub.clone())),
                choice(
                    "o",
                    "Open community in browser ↗",
                    Command::Native(path.clone()),
                ),
                choice(
                    "r",
                    "Mark community read",
                    Command::MarkCommunityRead(sub.clone()),
                ),
                read("a", "About and sidebar", Query::Community(sub.clone())),
                read("R", "Rules", Query::Rules(sub.clone())),
                read(
                    "wi",
                    "Wiki index",
                    Query::Wiki {
                        subreddit: sub.clone(),
                        page: "index".into(),
                    },
                ),
                choice(
                    "wp",
                    "Open wiki page",
                    Command::Input(form(
                        "Community wiki",
                        FormKind::Wiki(sub.clone()),
                        vec![field("Page", "index", false)],
                        &format!("{path}/wiki"),
                    )),
                ),
                read("mi", "Moderators", Query::Moderators(sub.clone())),
                native(
                    "mm",
                    "Message the mods",
                    &format!("{path}/about/moderators"),
                ),
                native("mq", "Moderation queue", &format!("{path}/about/modqueue")),
                native("mM", "Mod mail", "/mod/mail"),
            ],
            MenuScope::Community,
        );
        if let Some(Screen::Menu { native, .. }) = self.experience.screens.last_mut() {
            *native = path;
        }
    }
}
