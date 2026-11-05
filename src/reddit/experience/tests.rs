use super::*;
use crate::reddit::tests::{Fixture, Reply, saved_export};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

mod action_targets;
mod pages;
mod profile_requests;
mod request_validation_guards;
mod write_request_forms;

fn me() -> Reply {
    Reply::json(json!({"data":{"name":"reader","modhash":"fixture-csrf"}}))
}
fn read(reddit: &Reddit, query: Query) -> Response {
    reddit
        .experience(&Request::Read { query, after: None })
        .unwrap()
}

fn comment_target() -> Target {
    Target {
        id: "t1_comment".into(),
        author: "reader".into(),
        subreddit: "rust".into(),
        permalink: "/r/rust/comments/post/title/comment/".into(),
    }
}

fn action_thread() -> Value {
    json!([
        {"kind":"Listing","data":{"children":[{"kind":"t3","data":{
            "name":"t3_post","id":"post","author":"reader","subreddit":"rust",
            "permalink":"/r/rust/comments/post/title/","selftext":"post source",
            "saved":false,"likes":null,"hidden":true,"is_self":true
        }}]}},
        {"kind":"Listing","data":{"children":[{"kind":"t1","data":{
            "name":"t1_ancestor","id":"ancestor","author":"other","body":"ancestor",
            "replies":{"kind":"Listing","data":{"children":[{"kind":"t1","data":{
                "name":"t1_comment","id":"comment","author":"reader","subreddit":"rust",
                "permalink":"/r/rust/comments/post/title/comment/","body":">!spoiler!<\n**bold**",
                "saved":true,"likes":true
            }}]}}
        }}]}}
    ])
}
