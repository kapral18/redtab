use super::*;

mod admission;
mod fairness;
mod quota_hints;

fn quota(remaining: &str, reset: &str) -> (Rate, Url) {
    use crate::reddit::{
        tests::{Fixture, Reply},
        transport::{self, Transport},
    };
    let headers = vec![
        ("X-Ratelimit-Remaining".into(), remaining.to_owned()),
        ("X-Ratelimit-Reset".into(), reset.to_owned()),
    ];
    let fixture = Fixture::new(move |_| Reply {
        status: 200,
        body: String::new(),
        headers: headers.clone(),
    });
    let transport = Transport::new().unwrap();
    let response = transport::wait(transport.client.get(&fixture.base).send(), None)
        .unwrap()
        .unwrap();
    let rate = Rate::new(Duration::ZERO);
    let url = Url::parse(&fixture.base).unwrap();
    rate.observe(&response, &url, Instant::now(), 0, 0);
    (rate, url)
}

fn rapid_listings(rate: &Rate, url: &Url, count: usize) -> usize {
    for started in 0..count {
        let deadline = Instant::now() + Duration::from_millis(50);
        if rate
            .admit(url, Priority::Listing, true, || Instant::now() >= deadline)
            .is_err()
        {
            return started;
        }
    }
    count
}
