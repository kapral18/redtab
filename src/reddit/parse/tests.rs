use super::*;
use serde_json::json;

mod comments;
mod listings;
mod post_media;
mod posts;
mod replies;

fn focused_thread(post: &str, children: Value) -> Value {
    json!([
        {"kind":"Listing","data":{"children":[{"kind":"t3","data":{
            "id":post,"title":"post","subreddit":"rust","permalink":format!("/comments/{post}/")
        }}]}},
        {"kind":"Listing","data":{"children":children}}
    ])
}
