use super::*;

pub(super) fn ordered_choices(choices: Vec<Choice>, scope: MenuScope) -> Vec<Choice> {
    let mut ranked: Vec<_> = choices
        .into_iter()
        .map(|mut choice| {
            let (rank, section) = match scope {
                MenuScope::Reddit => match &choice.command {
                    Command::Read(Query::Inbox(_)) => (0, "Notifications"),
                    Command::Native(path) if path == "/notifications" => (0, "Notifications"),
                    Command::Native(path) if path == "/chat/" => (1, "Messages"),
                    Command::Read(Query::User { .. } | Query::Profile(_)) => (2, "Your account"),
                    Command::Search(_)
                    | Command::Input(Form {
                        kind: FormKind::Profile,
                        ..
                    }) => (3, "Search"),
                    Command::Read(Query::Multis | Query::Communities) | Command::Subreddit(_) => {
                        (4, "Explore")
                    }
                    Command::Input(_) => (5, "Create"),
                    _ => (6, "On Reddit"),
                },
                MenuScope::Content => match &choice.command {
                    Command::Input(Form {
                        kind: FormKind::Reply(_) | FormKind::Edit(_),
                        ..
                    }) => (0, "Reply and edit"),
                    Command::Write(Action::Vote { .. } | Action::Save { .. }) => {
                        (1, "Vote and save")
                    }
                    Command::Write(Action::Read { .. }) => (2, "Notifications"),
                    Command::Write(_) => (3, "Post controls"),
                    Command::Read(_) => (4, "Related"),
                    Command::Input(Form {
                        kind: FormKind::Report(_),
                        ..
                    }) => (5, "Report"),
                    Command::Confirm(_, _) => (7, "Destructive"),
                    _ => (6, "On Reddit"),
                },
                MenuScope::User => (0, "Profile"),
                MenuScope::Links => (0, "Links"),
                MenuScope::Community => match &choice.command {
                    Command::Subreddit(_) | Command::MarkCommunityRead(_) => (0, "Community"),
                    Command::Native(path) if path.strip_prefix("/r/").is_some_and(is_community) => {
                        (0, "Community")
                    }
                    Command::Read(Query::Community(_) | Query::Rules(_)) => (1, "About"),
                    Command::Read(Query::Wiki { .. }) | Command::Input(_) => (2, "Wiki"),
                    Command::Read(Query::Moderators(_)) => (3, "Moderators"),
                    _ => (4, "On Reddit"),
                },
                MenuScope::Comments => match &choice.command {
                    Command::Input(_) => (0, "Find"),
                    _ => (1, "Sort comments"),
                },
                MenuScope::Message => match &choice.command {
                    Command::Write(_) => (0, "Notifications"),
                    _ => (1, "On Reddit"),
                },
            };
            choice.section = section;
            (rank, choice)
        })
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, choice)| choice).collect()
}

pub(super) fn link_command(input: &str) -> anyhow::Result<Command> {
    let base = url::Url::parse("https://www.reddit.com/").unwrap();
    let input = input.trim();
    let url = if input.starts_with('/') {
        base.join(input)
    } else {
        url::Url::parse(input)
    }
    .map_err(|_| {
        anyhow::anyhow!("Enter a full Reddit URL or /r/name, /user/name or /comments/id")
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        anyhow::bail!("Enter an http or https Reddit link without credentials or a custom port");
    }
    let path = url.path();
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    match url.host_str() {
        Some("redd.it" | "www.redd.it") => {
            if let [id] = parts.as_slice() {
                let path = format!("/comments/{id}/");
                if crate::reddit::parse::post_id(&path).is_some() {
                    return Ok(Command::Read(Query::Thread(path)));
                }
            }
        }
        Some(
            "reddit.com" | "www.reddit.com" | "old.reddit.com" | "new.reddit.com" | "np.reddit.com"
            | "m.reddit.com" | "sh.reddit.com",
        ) => {
            if crate::reddit::parse::post_id(path).is_some() {
                return Ok(Command::Read(Query::Thread(path.into())));
            }
            match parts.as_slice() {
                ["r", sub] if is_subreddit_name(sub) => {
                    return Ok(Command::Subreddit((*sub).into()));
                }
                ["u" | "user", user] if name(user).is_ok() => {
                    return Ok(Command::User((*user).into()));
                }
                _ => {}
            }
        }
        _ => anyhow::bail!("This link is not on reddit.com or redd.it"),
    }
    anyhow::bail!(
        "Use a full post, comment, community or profile link; shortened /s/ share links are not supported"
    )
}

pub(super) fn search_choices() -> Vec<Choice> {
    [
        ("sp", "Search posts across Reddit", SearchKind::Posts),
        ("sc", "Search communities", SearchKind::Communities),
        ("sC", "Search comments", SearchKind::Comments),
        ("sm", "Search media posts", SearchKind::Media),
        ("su", "Search people", SearchKind::People),
    ]
    .into_iter()
    .map(|(shortcut, label, kind)| choice(shortcut, label, Command::Search(kind)))
    .collect()
}

impl App {
    pub fn reddit_menu(&mut self) {
        if self.experience.writing() {
            self.info("Wait for the pending Reddit action");
            return;
        }
        self.clear_experience();
        let mut choices = Vec::new();
        if let Some(account) = self.reddit.account() {
            choices.push(read(
                "ap",
                "My profile",
                Query::Profile(account.name.clone()),
            ));
            for (shortcut, label, feed) in [
                ("as", "Saved on Reddit", UserFeed::Saved),
                ("ah", "Hidden posts", UserFeed::Hidden),
                ("au", "Upvoted posts", UserFeed::Upvoted),
                ("ad", "Downvoted posts", UserFeed::Downvoted),
            ] {
                choices.push(read(
                    shortcut,
                    label,
                    Query::User {
                        name: account.name.clone(),
                        feed,
                    },
                ));
            }
        }
        for (shortcut, label, kind) in [
            ("nu", "Unread notifications", Inbox::Unread),
            ("nc", "Comment replies", Inbox::Replies),
            ("np", "Post replies", Inbox::PostReplies),
            ("nm", "Mentions", Inbox::Mentions),
            ("ni", "Inbox", Inbox::All),
        ] {
            choices.push(read(shortcut, label, Query::Inbox(kind)));
        }
        choices.extend([
            read("gf", "Custom feeds", Query::Multis),
            read("gc", "Discover communities", Query::Communities),
            choice("gp", "Popular posts", Command::Subreddit("popular".into())),
            choice("ga", "All Reddit", Command::Subreddit("all".into())),
        ]);
        choices.extend(search_choices());
        choices.extend([
            choice(
                "gu",
                "Open a user profile",
                Command::Input(form(
                    "User profile",
                    FormKind::Profile,
                    vec![field("Username", "", false)],
                    "/",
                )),
            ),
            choice(
                "ct",
                "Create text post",
                Command::Input(self.post_form(false)),
            ),
            choice(
                "cl",
                "Create link post",
                Command::Input(self.post_form(true)),
            ),
            choice(
                "cf",
                "Create a custom feed",
                Command::Input(form(
                    "Create private custom feed",
                    FormKind::CreateMulti,
                    vec![
                        field("Name", "", false),
                        Field {
                            required: false,
                            ..field("Description", "", true)
                        },
                    ],
                    "/",
                )),
            ),
            native("cm", "Upload media, create a poll or AMA", "/submit"),
            native("mc", "Chat", "/chat/"),
            native("nn", "Notifications", "/notifications"),
            native("af", "Following and favorite communities", "/"),
            native(
                "pp",
                "Privacy, blocked users and profile settings",
                "/settings/privacy",
            ),
            native("pn", "Notification settings", "/settings/notifications"),
            native("mq", "Moderation queue", "/mod/queue"),
            native("mm", "Mod mail", "/mod/mail"),
        ]);
        self.experience.screens.push(Screen::Menu {
            title: "Reddit".into(),
            choices: ordered_choices(choices, MenuScope::Reddit),
            selected: 0,
            native: "/".into(),
        });
        self.overlay = Some(Overlay::Experience);
    }

    pub(super) fn post_form(&self, link: bool) -> Form {
        let sub = self.experience_community();
        form(
            if link {
                "Create link post"
            } else {
                "Create text post"
            },
            FormKind::Submit { link },
            vec![
                field("Subreddit", &sub, false),
                field("Title", "", false),
                field(if link { "URL" } else { "Body (Markdown)" }, "", !link),
            ],
            &if sub.is_empty() {
                "/submit".into()
            } else {
                format!("/r/{sub}/submit")
            },
        )
    }

    pub fn show_actions(&mut self) {
        self.cancel_collection_refresh();
        if self.experience.pending.is_some() {
            self.info("Wait for the pending Reddit request");
            return;
        }
        let Some(target) = self.action_target() else {
            self.community_menu();
            return;
        };
        if target.id.starts_with("t4_") {
            let choices = vec![
                choice(
                    "r",
                    "Mark read",
                    Command::Write(Action::Read {
                        id: target.id.clone(),
                        read: true,
                    }),
                ),
                choice(
                    "u",
                    "Mark unread",
                    Command::Write(Action::Read {
                        id: target.id,
                        read: false,
                    }),
                ),
                native("o", "Notifications", "/notifications"),
            ];
            self.push_menu("Message actions", choices, MenuScope::Message);
        } else {
            self.read_experience(Query::Inspect(target), None);
        }
    }

    pub fn quick_action(&mut self, shortcut: &'static str) {
        self.cancel_collection_refresh();
        if self.experience.pending.is_some() {
            self.info("Wait for the pending Reddit request");
            return;
        }
        let Some(target) = self.action_target() else {
            return;
        };
        if target.id.starts_with("t4_") {
            self.show_actions();
            return;
        }
        self.read_experience(Query::Inspect(target), None);
        self.experience.quick = Some(shortcut);
    }

    pub(super) fn push_menu(&mut self, title: &str, choices: Vec<Choice>, scope: MenuScope) {
        let native = self.experience_native_path();
        self.experience.error = None;
        self.experience.screens.push(Screen::Menu {
            title: title.into(),
            choices: ordered_choices(choices, scope),
            selected: 0,
            native,
        });
        self.overlay = Some(Overlay::Experience);
    }

    pub(super) fn multi_action(&mut self, key: KeyCode) {
        let Some(Screen::Browse {
            query: Query::Multi { owner, name },
            ..
        }) = self.experience.screens.last()
        else {
            return;
        };
        let (owner, name) = (owner.clone(), name.clone());
        if key == KeyCode::Char('D') {
            let native = self.experience_native_path();
            self.experience.screens.push(Screen::Confirm {
                label: format!("Delete custom feed {name} from Reddit?"),
                action: Action::DeleteMulti { owner, name },
                native,
            });
        } else {
            let add = key == KeyCode::Char('a');
            self.experience.screens.push(Screen::Input(form(
                if add {
                    "Add community to custom feed"
                } else {
                    "Remove community from custom feed"
                },
                FormKind::MultiCommunity { owner, name, add },
                vec![field("Subreddit", "", false)],
                "/",
            )));
        }
    }
}
