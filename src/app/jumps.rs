use super::*;

const LIMIT: usize = 100;

#[derive(Clone)]
pub(super) struct Location {
    tab: u64,
    post: Option<Post>,
}

impl Location {
    fn same(&self, other: &Location) -> bool {
        self.tab == other.tab
            && self.post.as_ref().map(|post| &post.permalink)
                == other.post.as_ref().map(|post| &post.permalink)
    }
}

#[derive(Default)]
pub struct Jumps {
    list: Vec<Location>,
    index: usize,
    current: Option<Location>,
}

impl Jumps {
    fn push(&mut self, location: Location) {
        self.list.retain(|entry| !entry.same(&location));
        self.list.push(location);
        if self.list.len() > LIMIT {
            self.list.remove(0);
        }
        self.index = self.list.len();
    }

    fn settle(&mut self, here: Location) {
        if let Some(current) = self.current.take()
            && !current.same(&here)
        {
            self.push(current);
        }
        self.current = Some(here);
    }

    fn retain(&mut self, keep: impl Fn(&Location) -> bool) {
        let end = self.index.min(self.list.len());
        self.index = self.list[..end].iter().filter(|entry| keep(entry)).count();
        self.list.retain(keep);
    }

    fn older(&mut self, here: Location) -> Option<Location> {
        if self.index >= self.list.len() {
            self.push(here);
            self.index = self.list.len() - 1;
        } else if self.list[self.index].same(&here) {
            self.list[self.index] = here;
        }
        self.index = self.index.checked_sub(1)?;
        Some(self.list[self.index].clone())
    }

    fn newer(&mut self) -> Option<Location> {
        if self.index + 1 >= self.list.len() {
            return None;
        }
        self.index += 1;
        Some(self.list[self.index].clone())
    }
}

impl App {
    fn location(&self) -> Location {
        Location {
            tab: self.active_tab().id,
            post: self
                .post_focused()
                .then(|| self.post_view.as_ref().map(|view| view.post.clone()))
                .flatten(),
        }
    }

    /// A key that leaves the main view somewhere else records where it was.
    pub fn settle_jump(&mut self) {
        if self.overlay.is_none() && self.focus != Focus::Sidebar && self.ancestor_follow.is_none()
        {
            let here = self.location();
            self.jumps.settle(here);
        }
    }

    pub fn jump(&mut self, forward: bool) {
        let here = self.location();
        self.jumps.settle(here.clone());
        let ids: HashSet<u64> = self.tabs.iter().map(|tab| tab.id).collect();
        self.jumps.retain(|entry| ids.contains(&entry.tab));
        let target = if forward {
            self.jumps.newer()
        } else {
            self.jumps.older(here)
        };
        let Some(target) = target else {
            return;
        };
        let Some(index) = self.tabs.iter().position(|tab| tab.id == target.tab) else {
            return;
        };
        if self.focus == Focus::Sidebar {
            self.focus_main();
        }
        if index != self.active {
            self.enter_tab(index, false);
        }
        match target.post {
            // A post of Posts results does not cover the Communities or People rows.
            Some(_) if index == super::SEARCH_TAB && self.search.shows_rows() => self.close_post(),
            Some(post) => {
                if self
                    .post_view
                    .as_ref()
                    .is_some_and(|view| view.post.permalink == post.permalink)
                {
                    self.focus_open_post();
                } else {
                    let permalink = post.permalink.clone();
                    self.open_post_permalink(post, permalink);
                }
            }
            None if self.post_focused() && self.detail_fits() => self.focus = Focus::List,
            None if self.post_focused() => self.close_post(),
            None => {}
        }
        self.jumps.current = Some(self.location());
    }
}
